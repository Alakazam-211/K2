//! The compute history trimmer (0313): ended jobs stop carrying their
//! whole plan for good.
//!
//! A job's `plan_json` held its full argv and every `--env` value forever:
//! on a busy controller 33k jobs came to 830 MB of plans (git bundles
//! passed as base64 env, long `bash -c` scripts), read by every scan. A
//! job that ended more than [`TRIM_AFTER_SECS`] ago keeps a summary: its
//! argv's first [`ARGV_KEEP_BYTES`], env keys with a marker for each value
//! ([`store::env_marker`]), and every other field as it was (cache, needs,
//! limits, `k2ctl`), so the plan still reads and every view of it works.
//! Its kept env values go too ([`store::ENV_KEEP_AFTER_END_SECS`], the
//! same age), as do any left by a job that is gone.
//!
//! - **Small batches, never one big write.** [`trim_batch`] reads up to
//!   [`TRIM_BATCH`] jobs on the read pool, works out each summary with no
//!   lock held, then writes them in one short transaction. The daemon's
//!   loop ([`run_pass`]) pauses between batches.
//! - **Resumable.** Its place (`created_at`, id) is saved with each batch
//!   (`compute_admin_settings`), so a restart picks up where it stopped.
//! - **Never an open job, never a changed row.** Only ended states; a row
//!   whose plan changed since it was read (a late outputs report) is left
//!   for the next pass.
//! - **Signed evidence keeps its plan** ([`store::KEEPS_INPUTS_SQL`]): a
//!   release re-hashes it against the receipt.
//! - No `VACUUM`: SQLite reuses the freed pages for new rows.

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;

use super::store;

/// A job is trimmed once it ended this long ago: when its env values go
/// (a summary can't be re-hashed or run again anyway).
pub const TRIM_AFTER_SECS: i64 = store::ENV_KEEP_AFTER_END_SECS;
/// Jobs per batch (one short write transaction).
pub const TRIM_BATCH: usize = 200;
/// How much of a trimmed job's argv stays.
pub const ARGV_KEEP_BYTES: usize = 1024;
/// The top-level key a trimmed plan carries (`{"at", "bytes"}`): `Plan`
/// ignores it, like `k2ctl`.
pub const TRIMMED_KEY: &str = "k2trimmed";
/// Where the trimmer's place is kept (`compute_admin_settings`).
pub const CURSOR_KEY: &str = "history_trim_cursor";

const ENDED: &str = "('done','failed','cancelled','timeout','interrupted','unknown')";

/// What one batch did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Batch {
    /// Ended jobs read.
    pub read: usize,
    /// Plans written as summaries.
    pub trimmed: usize,
    /// `compute_job_env` rows deleted.
    pub env_rows: usize,
    /// Plan bytes before and after, over the trimmed rows.
    pub bytes_before: u64,
    pub bytes_after: u64,
    /// No more jobs to read past this batch (the pass is over).
    pub done: bool,
}

/// The summary of a plan: argv cut to [`ARGV_KEEP_BYTES`], env values as
/// markers, [`TRIMMED_KEY`] set. `None` when it is already a summary or
/// isn't a plan object.
pub fn trim_plan_json(plan_json: &str, at: i64) -> Option<String> {
    let mut v: Value = serde_json::from_str(plan_json).ok()?;
    let m = v.as_object_mut()?;
    if m.contains_key(TRIMMED_KEY) {
        return None;
    }
    if let Some(env) = m.get_mut("env").and_then(Value::as_object_mut) {
        for val in env.values_mut() {
            if let Some(text) = val.as_str().filter(|t| !store::is_env_marker(t)) {
                *val = Value::String(store::env_marker(text));
            }
        }
    }
    if let Some(argv) = m.get_mut("argv").and_then(Value::as_array_mut) {
        let mut left = ARGV_KEEP_BYTES;
        let mut kept = Vec::new();
        for a in argv.iter() {
            let Some(text) = a.as_str() else { continue };
            if text.len() <= left {
                left -= text.len();
                kept.push(Value::String(text.to_string()));
                continue;
            }
            let mut cut = left;
            while !text.is_char_boundary(cut) {
                cut -= 1;
            }
            kept.push(Value::String(format!("{}…[trimmed {} bytes]", &text[..cut], text.len() - cut)));
            break;
        }
        let total = argv.len();
        if kept.len() < total {
            let dropped = total - kept.len();
            kept.push(Value::String(format!("…[{dropped} more args trimmed]")));
        }
        *argv = kept;
    }
    m.insert(TRIMMED_KEY.into(), serde_json::json!({"at": at, "bytes": plan_json.len()}));
    serde_json::to_string(&v).ok()
}

fn cursor(conn: &Connection) -> (i64, String) {
    let raw: Option<String> =
        conn.query_row("SELECT value FROM compute_admin_settings WHERE key = ?1", params![CURSOR_KEY], |r| r.get(0)).optional().ok().flatten();
    raw.and_then(|r| {
        let (c, id) = r.split_once(':')?;
        Some((c.parse().ok()?, id.to_string()))
    })
    .unwrap_or((i64::MIN, String::new()))
}

fn save_cursor(conn: &Connection, at: (i64, &str), now: i64) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO compute_admin_settings (key, value, updated_by, updated_at) VALUES (?1, ?2, 'k2', ?3) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
        params![CURSOR_KEY, format!("{}:{}", at.0, at.1), now],
    )
    .map(|_| ())
}

/// Start the next pass from the beginning (the loop does this once a pass
/// is over: a job that ended late, after the place passed it, is read then).
pub fn reset_cursor(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM compute_admin_settings WHERE key = ?1", params![CURSOR_KEY]).map(|_| ())
}

/// Env rows that can go: an ended job's past
/// [`store::ENV_KEEP_AFTER_END_SECS`], a gone job's. Never signed
/// evidence's.
fn spent_env(conn: &Connection, now: i64, limit: usize) -> Vec<String> {
    let sql = format!(
        "SELECT DISTINCT e.job_id FROM compute_job_env e LEFT JOIN compute_jobs j ON j.id = e.job_id \
         WHERE j.id IS NULL OR (NOT {keeps} AND j.state IN {ENDED} AND j.ended_at IS NOT NULL AND j.ended_at < ?1) LIMIT ?2",
        keeps = store::KEEPS_INPUTS_SQL.replace("plan_json", "j.plan_json"),
    );
    let Ok(mut st) = conn.prepare(&sql) else { return Vec::new() };
    st.query_map(params![now - store::ENV_KEEP_AFTER_END_SECS, limit as i64], |r| r.get::<_, String>(0))
        .map(|rows| rows.filter_map(Result::ok).collect())
        .unwrap_or_default()
}

/// One batch at `now` (see the module docs). `Err` only when the write
/// failed (nothing of the batch is written then).
pub fn trim_batch(now: i64) -> Result<Batch, String> {
    let cutoff = now - TRIM_AFTER_SECS;
    // Read (the pool: no lock): the next ended jobs past the place, then
    // each one's plan, and the env rows that can go.
    let (rows, env_ids, last) = crate::db::read(|conn| {
        let (c, id) = cursor(conn);
        // Index only (0312's state, created_at, id): no plan read here.
        let ids: Vec<(String, i64)> = conn
            .prepare(&format!(
                "SELECT id, created_at FROM compute_jobs WHERE state IN {ENDED} AND created_at < ?1 \
                 AND (created_at > ?2 OR (created_at = ?2 AND id > ?3)) ORDER BY created_at, id LIMIT ?4"
            ))
            .and_then(|mut st| {
                st.query_map(params![cutoff, c, id, TRIM_BATCH as i64], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()
            })
            .unwrap_or_default();
        let last = ids.last().map(|(id, c)| (*c, id.clone()));
        let mut rows = Vec::new();
        for (id, _) in &ids {
            let row: Option<(String, Option<i64>, bool)> = conn
                .query_row(
                    &format!("SELECT plan_json, ended_at, {} FROM compute_jobs WHERE id = ?1", store::KEEPS_INPUTS_SQL),
                    params![id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()
                .ok()
                .flatten();
            if let Some((plan, ended, keeps)) = row {
                if !keeps && ended.is_some_and(|e| e < cutoff) {
                    rows.push((id.clone(), plan));
                }
            }
        }
        (rows, spent_env(conn, now, TRIM_BATCH), last)
    });
    let mut out = Batch { read: rows.len(), done: last.is_none(), ..Batch::default() };
    // Work (no lock): each summary.
    let trims: Vec<(String, String, String)> =
        rows.into_iter().filter_map(|(id, plan)| trim_plan_json(&plan, now).map(|t| (id, plan, t))).collect();
    // Write (the lock, one short transaction).
    let db = crate::db::shared();
    let conn = db.lock();
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    for (id, old, new) in &trims {
        let n = tx
            .execute(
                &format!("UPDATE compute_jobs SET plan_json = ?2 WHERE id = ?1 AND state IN {ENDED} AND plan_json = ?3"),
                params![id, new, old],
            )
            .map_err(|e| e.to_string())?;
        if n == 1 {
            out.trimmed += 1;
            out.bytes_before += old.len() as u64;
            out.bytes_after += new.len() as u64;
            out.env_rows += tx
                .execute(&format!("DELETE FROM compute_job_env WHERE job_id = ?1 AND NOT EXISTS (SELECT 1 FROM compute_jobs WHERE id = ?1 AND {})", store::KEEPS_INPUTS_SQL), params![id])
                .map_err(|e| e.to_string())?;
        }
    }
    if !env_ids.is_empty() {
        // Checked again under the lock: still spent (a retry may have
        // brought a job back since the read). The table holds only jobs
        // that may still run, so this read is small.
        let still: std::collections::HashSet<String> = spent_env(&tx, now, usize::MAX).into_iter().collect();
        for id in env_ids.iter().filter(|id| still.contains(*id)) {
            out.env_rows += store::drop_job_env(&tx, id)?;
        }
    }
    if let Some((c, id)) = &last {
        save_cursor(&tx, (*c, id), now).map_err(|e| e.to_string())?;
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(out)
}

/// One full pass, `pause` between batches; logs its progress. Returns the
/// totals. The daemon runs it on its own thread ([`spawn`]).
pub fn run_pass(now: impl Fn() -> i64, pause: std::time::Duration) -> Batch {
    let mut total = Batch::default();
    let mut batches = 0u32;
    loop {
        match trim_batch(now()) {
            Ok(b) => {
                batches += 1;
                total.read += b.read;
                total.trimmed += b.trimmed;
                total.env_rows += b.env_rows;
                total.bytes_before += b.bytes_before;
                total.bytes_after += b.bytes_after;
                if b.trimmed > 0 && batches % 10 == 0 {
                    crate::log_debug!(
                        "[compute] history trim: {} plans trimmed so far ({} MB → {} MB), {} env rows dropped",
                        total.trimmed,
                        total.bytes_before >> 20,
                        total.bytes_after >> 20,
                        total.env_rows
                    );
                }
                if b.done {
                    total.done = true;
                    break;
                }
            }
            Err(e) => {
                crate::log_debug!("[compute] history trim: a batch failed ({e}); it runs again next pass");
                break;
            }
        }
        std::thread::sleep(pause);
    }
    if total.trimmed > 0 || total.env_rows > 0 {
        crate::log_debug!(
            "[compute] history trim: pass over: {} plans trimmed ({} MB → {} MB), {} env rows dropped",
            total.trimmed,
            total.bytes_before >> 20,
            total.bytes_after >> 20,
            total.env_rows
        );
    }
    if total.done {
        let db = crate::db::shared();
        let conn = db.lock();
        let _ = reset_cursor(&conn);
    }
    total
}

/// Pause between batches.
pub const BATCH_PAUSE: std::time::Duration = std::time::Duration::from_millis(500);
/// Wait between passes (and before the first, after boot).
pub const PASS_EVERY: std::time::Duration = std::time::Duration::from_secs(3600);

/// Start the trimmer's loop (once, at boot). `K2_COMPUTE_TRIM=0` turns it
/// off.
pub fn spawn() {
    if std::env::var("K2_COMPUTE_TRIM").is_ok_and(|v| v.trim() == "0") {
        return;
    }
    let _ = std::thread::Builder::new().name("compute-history-trim".into()).spawn(|| {
        // Let boot and the first placements settle.
        std::thread::sleep(std::time::Duration::from_secs(120));
        loop {
            run_pass(super::now, BATCH_PAUSE);
            std::thread::sleep(PASS_EVERY);
        }
    });
}

#[cfg(test)]
#[path = "trim_tests.rs"]
mod tests;
