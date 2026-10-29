//! The update feed's records in k2.db (migration 0255, TC-7).
//!
//! Plain rusqlite on the caller's connection; the daemon holds the lock.
//! Rows name the runtime App by its App id.

use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::Serialize;

pub const STATE_UPLOADING: &str = "uploading";
pub const STATE_LIVE: &str = "live";
pub const STATE_FAILED: &str = "failed";
pub const STATE_YANKED: &str = "yanked";

pub const UP_PENDING: &str = "pending";
pub const UP_UPLOADED: &str = "uploaded";
pub const UP_FAILED: &str = "failed";

pub const ACT_PUSH: &str = "push";
pub const ACT_PROMOTE: &str = "promote";
pub const ACT_ROLLBACK: &str = "rollback";
pub const ACT_PULL: &str = "pull";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyRow {
    pub app_id: String,
    pub pubkey: String,
    pub key_id: String,
    pub set_by: String,
    pub set_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseRow {
    pub id: String,
    pub app_id: String,
    pub version: String,
    /// The channel it was first pushed to.
    pub channel: String,
    pub notes: Option<String>,
    pub pub_date: String,
    pub created_by: String,
    pub created_at: i64,
    pub state: String,
    pub error: Option<String>,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactRow {
    pub release_id: String,
    pub platform: String,
    pub file_name: String,
    pub content_type: String,
    pub sha256: String,
    pub size: i64,
    /// The base64 `.sig` text (what Tauri's updater reads).
    pub signature: String,
    pub url: Option<String>,
    pub upload_state: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelRow {
    pub channel: String,
    pub release_id: String,
    pub updated_by: String,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckinRow {
    pub day: String,
    pub channel: String,
    pub platform: String,
    pub version: String,
    pub checks: i64,
    pub installs: i64,
}

// ── keys ─────────────────────────────────────────────────────────────

pub fn key(conn: &Connection, app: &str) -> rusqlite::Result<Option<KeyRow>> {
    conn.query_row("SELECT app_id, pubkey, key_id, set_by, set_at FROM app_update_keys WHERE app_id = ?1", params![app], |r| {
        Ok(KeyRow { app_id: r.get(0)?, pubkey: r.get(1)?, key_id: r.get(2)?, set_by: r.get(3)?, set_at: r.get(4)? })
    })
    .optional()
}

pub fn set_key(conn: &Connection, app: &str, pubkey: &str, key_id: &str, by: &str, now: i64) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO app_update_keys (app_id, pubkey, key_id, set_by, set_at) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT (app_id) DO UPDATE SET pubkey = excluded.pubkey, key_id = excluded.key_id, set_by = excluded.set_by, set_at = excluded.set_at",
        params![app, pubkey, key_id, by, now],
    )?;
    Ok(())
}

// ── releases ─────────────────────────────────────────────────────────

const REL_COLS: &str = "id, app_id, version, channel, notes, pub_date, created_by, created_at, state, error, updated_at";

fn rel(r: &Row<'_>) -> rusqlite::Result<ReleaseRow> {
    Ok(ReleaseRow {
        id: r.get(0)?,
        app_id: r.get(1)?,
        version: r.get(2)?,
        channel: r.get(3)?,
        notes: r.get(4)?,
        pub_date: r.get(5)?,
        created_by: r.get(6)?,
        created_at: r.get(7)?,
        state: r.get(8)?,
        error: r.get(9)?,
        updated_at: r.get(10)?,
    })
}

pub struct NewRelease<'a> {
    pub app_id: &'a str,
    pub version: &'a str,
    pub channel: &'a str,
    pub notes: Option<&'a str>,
    pub pub_date: &'a str,
    pub created_by: &'a str,
}

/// A new `uploading` release (a `failed` one of the same version is
/// replaced: a retry of a push that never went live).
pub fn insert_release(conn: &Connection, n: &NewRelease<'_>, now: i64) -> rusqlite::Result<ReleaseRow> {
    conn.execute("DELETE FROM app_update_releases WHERE app_id = ?1 AND version = ?2 AND state = 'failed'", params![n.app_id, n.version])?;
    let id = format!("rel-{}", &uuid::Uuid::new_v4().simple().to_string()[..16]);
    conn.execute(
        "INSERT INTO app_update_releases (id, app_id, version, channel, notes, pub_date, created_by, created_at, state, error, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'uploading', NULL, ?8)",
        params![id, n.app_id, n.version, n.channel, n.notes, n.pub_date, n.created_by, now],
    )?;
    release(conn, &id).map(|r| r.expect("the row just inserted"))
}

pub fn release(conn: &Connection, id: &str) -> rusqlite::Result<Option<ReleaseRow>> {
    conn.query_row(&format!("SELECT {REL_COLS} FROM app_update_releases WHERE id = ?1"), params![id], rel).optional()
}

pub fn release_by_version(conn: &Connection, app: &str, version: &str) -> rusqlite::Result<Option<ReleaseRow>> {
    conn.query_row(&format!("SELECT {REL_COLS} FROM app_update_releases WHERE app_id = ?1 AND version = ?2"), params![app, version], rel).optional()
}

/// Every release of the App, newest first (by creation).
pub fn releases(conn: &Connection, app: &str) -> rusqlite::Result<Vec<ReleaseRow>> {
    let mut st = conn.prepare(&format!("SELECT {REL_COLS} FROM app_update_releases WHERE app_id = ?1 ORDER BY created_at DESC, rowid DESC"))?;
    let rows = st.query_map(params![app], rel)?.collect();
    rows
}

pub fn set_release_state(conn: &Connection, id: &str, state: &str, error: Option<&str>, now: i64) -> rusqlite::Result<()> {
    conn.execute("UPDATE app_update_releases SET state = ?2, error = ?3, updated_at = ?4 WHERE id = ?1", params![id, state, error, now])?;
    Ok(())
}

/// At boot: every `uploading` release becomes `failed` (the daemon stopped
/// mid-upload; no channel moved). Returns `(app, version, id)` of each.
pub fn fail_interrupted(conn: &Connection, now: i64) -> rusqlite::Result<Vec<(String, String, String)>> {
    let rows: Vec<(String, String, String)> = {
        let mut st = conn.prepare("SELECT app_id, version, id FROM app_update_releases WHERE state = 'uploading'")?;
        let v = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<rusqlite::Result<_>>()?;
        v
    };
    for (_, _, id) in &rows {
        set_release_state(conn, id, STATE_FAILED, Some("the daemon stopped while uploading; push again"), now)?;
        conn.execute("UPDATE app_update_artifacts SET upload_state = 'failed' WHERE release_id = ?1 AND upload_state = 'pending'", params![id])?;
    }
    Ok(rows)
}

// ── artifacts ────────────────────────────────────────────────────────

pub fn insert_artifact(conn: &Connection, a: &ArtifactRow) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO app_update_artifacts (release_id, platform, file_name, content_type, sha256, size, signature, url, upload_state, error)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![a.release_id, a.platform, a.file_name, a.content_type, a.sha256, a.size, a.signature, a.url, a.upload_state, a.error],
    )?;
    Ok(())
}

pub fn artifacts(conn: &Connection, release_id: &str) -> rusqlite::Result<Vec<ArtifactRow>> {
    let mut st = conn.prepare(
        "SELECT release_id, platform, file_name, content_type, sha256, size, signature, url, upload_state, error
         FROM app_update_artifacts WHERE release_id = ?1 ORDER BY platform",
    )?;
    let rows = st
        .query_map(params![release_id], |r| {
            Ok(ArtifactRow {
                release_id: r.get(0)?,
                platform: r.get(1)?,
                file_name: r.get(2)?,
                content_type: r.get(3)?,
                sha256: r.get(4)?,
                size: r.get(5)?,
                signature: r.get(6)?,
                url: r.get(7)?,
                upload_state: r.get(8)?,
                error: r.get(9)?,
            })
        })?
        .collect();
    rows
}

pub fn set_artifact_upload(conn: &Connection, release_id: &str, platform: &str, state: &str, url: Option<&str>, error: Option<&str>) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE app_update_artifacts SET upload_state = ?3, url = COALESCE(?4, url), error = ?5 WHERE release_id = ?1 AND platform = ?2",
        params![release_id, platform, state, url, error],
    )?;
    Ok(())
}

// ── channels ─────────────────────────────────────────────────────────

pub fn channel(conn: &Connection, app: &str, ch: &str) -> rusqlite::Result<Option<ChannelRow>> {
    conn.query_row(
        "SELECT channel, release_id, updated_by, updated_at FROM app_update_channels WHERE app_id = ?1 AND channel = ?2",
        params![app, ch],
        |r| Ok(ChannelRow { channel: r.get(0)?, release_id: r.get(1)?, updated_by: r.get(2)?, updated_at: r.get(3)? }),
    )
    .optional()
}

pub fn channels(conn: &Connection, app: &str) -> rusqlite::Result<Vec<ChannelRow>> {
    let mut st = conn.prepare("SELECT channel, release_id, updated_by, updated_at FROM app_update_channels WHERE app_id = ?1 ORDER BY channel")?;
    let rows = st
        .query_map(params![app], |r| Ok(ChannelRow { channel: r.get(0)?, release_id: r.get(1)?, updated_by: r.get(2)?, updated_at: r.get(3)? }))?
        .collect();
    rows
}

/// Point `ch` at `release_id` (`None`: the channel is empty) and log it.
pub fn set_channel(conn: &Connection, app: &str, ch: &str, release_id: Option<&str>, action: &str, by: &str, now: i64) -> rusqlite::Result<()> {
    match release_id {
        Some(id) => conn.execute(
            "INSERT INTO app_update_channels (app_id, channel, release_id, updated_by, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (app_id, channel) DO UPDATE SET release_id = excluded.release_id, updated_by = excluded.updated_by, updated_at = excluded.updated_at",
            params![app, ch, id, by, now],
        )?,
        None => conn.execute("DELETE FROM app_update_channels WHERE app_id = ?1 AND channel = ?2", params![app, ch])?,
    };
    conn.execute(
        "INSERT INTO app_update_channel_log (app_id, channel, release_id, action, by, at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![app, ch, release_id, action, by, now],
    )?;
    Ok(())
}

/// The releases a channel has pointed at, oldest move first (one entry per
/// move; a release can appear twice).
pub fn channel_history(conn: &Connection, app: &str, ch: &str) -> rusqlite::Result<Vec<String>> {
    let mut st = conn.prepare("SELECT release_id FROM app_update_channel_log WHERE app_id = ?1 AND channel = ?2 AND release_id IS NOT NULL ORDER BY id")?;
    let rows = st.query_map(params![app, ch], |r| r.get(0))?.collect();
    rows
}

/// Every channel name the App has ever used (for a pull's fallback).
pub fn channel_names(conn: &Connection, app: &str) -> rusqlite::Result<Vec<String>> {
    let mut st = conn.prepare("SELECT DISTINCT channel FROM app_update_channel_log WHERE app_id = ?1 ORDER BY channel")?;
    let rows = st.query_map(params![app], |r| r.get(0))?.collect();
    rows
}

/// Apps with a key, a release or a channel (the serve snapshot and the
/// fleet view).
pub fn apps_with_feed(conn: &Connection) -> rusqlite::Result<Vec<String>> {
    let mut st = conn.prepare("SELECT app_id FROM app_update_keys UNION SELECT DISTINCT app_id FROM app_update_releases ORDER BY 1")?;
    let rows = st.query_map([], |r| r.get(0))?.collect();
    rows
}

// ── check-ins (counts only) ──────────────────────────────────────────

/// Add `checks` version checks, and `installs` new distinct installs, to a
/// day's row.
pub fn add_checkins(conn: &Connection, app: &str, day: &str, ch: &str, platform: &str, version: &str, checks: i64, installs: i64) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO app_update_checkins (app_id, day, channel, platform, version, checks, installs) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT (app_id, day, channel, platform, version) DO UPDATE SET checks = checks + excluded.checks, installs = installs + excluded.installs",
        params![app, day, ch, platform, version, checks, installs],
    )?;
    Ok(())
}

/// Record an install id's hash for the day: `true` = first time today.
pub fn install_seen_first(conn: &Connection, app: &str, day: &str, hash: &str) -> rusqlite::Result<bool> {
    let n = conn.execute("INSERT OR IGNORE INTO app_update_install_days (app_id, day, install_hash) VALUES (?1, ?2, ?3)", params![app, day, hash])?;
    Ok(n == 1)
}

/// Drop install hashes older than `keep_from` (a `YYYY-MM-DD`).
pub fn prune_install_days(conn: &Connection, keep_from: &str) -> rusqlite::Result<usize> {
    conn.execute("DELETE FROM app_update_install_days WHERE day < ?1", params![keep_from])
}

/// Check-in rows from `from_day` on, oldest first.
pub fn checkins(conn: &Connection, app: &str, from_day: &str) -> rusqlite::Result<Vec<CheckinRow>> {
    let mut st = conn.prepare(
        "SELECT day, channel, platform, version, checks, installs FROM app_update_checkins
         WHERE app_id = ?1 AND day >= ?2 ORDER BY day, channel, platform, version",
    )?;
    let rows = st
        .query_map(params![app, from_day], |r| {
            Ok(CheckinRow { day: r.get(0)?, channel: r.get(1)?, platform: r.get(2)?, version: r.get(3)?, checks: r.get(4)?, installs: r.get(5)? })
        })?
        .collect();
    rows
}

/// The App went away: its key, releases, channels and counts.
pub fn forget_app(conn: &Connection, app: &str) -> rusqlite::Result<()> {
    for sql in [
        "DELETE FROM app_update_channels WHERE app_id = ?1",
        "DELETE FROM app_update_channel_log WHERE app_id = ?1",
        "DELETE FROM app_update_artifacts WHERE release_id IN (SELECT id FROM app_update_releases WHERE app_id = ?1)",
        "DELETE FROM app_update_releases WHERE app_id = ?1",
        "DELETE FROM app_update_keys WHERE app_id = ?1",
        "DELETE FROM app_update_checkins WHERE app_id = ?1",
        "DELETE FROM app_update_install_days WHERE app_id = ?1",
    ] {
        conn.execute(sql, params![app])?;
    }
    Ok(())
}
