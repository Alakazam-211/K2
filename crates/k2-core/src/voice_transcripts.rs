//! Agents review the transcripts of this server's own voice rooms (K2
//! 0.46.11, Rosson 2026-11-10: "recordings that have saved transcripts that
//! agents can review after our calls"). The store; the routes are
//! `k2-daemon`'s `voice::transcripts`.
//!
//! - **The switch** "Read call transcripts" (`voice.transcripts-read`), per
//!   agent, off by default. Only the server's owner turns it on (a
//!   permission card or Settings); the owner or an Admin turns it off.
//! - **Scope.** Recordings in the K5 rooms only (the K2 Admin Space). An
//!   App's Spaces keep their own rule (`server_read`, `tool_hook::recording`).
//! - **Every read is logged** here ([`log_read`]: who, which recording, how
//!   many lines), beside Sew's own `recording.transcript_read`.
//! - **After the call.** A room can send a note to chosen agents when a
//!   transcript is ready ([`subscribe`]); opt-in per room and per agent.
//!
//! The tables live in skin.db (idempotent DDL like the other Sew 0.9.0
//! tables; no numbered migration). None decides a pass, so none is an
//! access table (`skin_access_live`).

use std::collections::HashSet;

use rusqlite::{params, Connection, OptionalExtension};

/// The permission card's action id (Owner floor).
pub const ACTION: &str = "voice.transcripts-read";
/// The `/cli/agent-access/set` toggle that writes the switch.
pub const TOGGLE: &str = "voice_transcripts";
/// The refusal an agent without the switch gets.
pub const OFF_CODE: &str = "transcripts_read_off";
/// The line K2's own recording notice shows while any agent holds the switch.
pub const AGENT_READERS_LINE: &str = "Agents on this server may read the transcript.";

/// Idempotent DDL, called from `skin::open_db`.
pub fn ensure_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS voice_transcript_readers (
            workspace_id TEXT PRIMARY KEY NOT NULL,
            set_by       TEXT NOT NULL,
            set_at       INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS voice_transcript_reads (
            id           INTEGER PRIMARY KEY AUTOINCREMENT,
            at           INTEGER NOT NULL,
            reader       TEXT NOT NULL,
            workspace_id TEXT,
            recording_id TEXT NOT NULL,
            lines        INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS voice_transcript_subscriptions (
            channel_id   TEXT NOT NULL,
            workspace_id TEXT NOT NULL,
            set_by       TEXT NOT NULL,
            set_at       INTEGER NOT NULL,
            PRIMARY KEY (channel_id, workspace_id)
         );
         CREATE TABLE IF NOT EXISTS voice_transcript_notified (
            recording_id TEXT NOT NULL,
            workspace_id TEXT NOT NULL,
            at           INTEGER NOT NULL,
            PRIMARY KEY (recording_id, workspace_id)
         );
         CREATE TABLE IF NOT EXISTS voice_transcript_cursor (
            space_id TEXT PRIMARY KEY NOT NULL,
            after    TEXT
         );
         CREATE TABLE IF NOT EXISTS voice_room_consent_mode (
            channel_id TEXT PRIMARY KEY NOT NULL,
            set_at     INTEGER NOT NULL
         );",
    )
    .map_err(|e| format!("skin.db transcript schema: {e}"))
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

fn store_exists() -> bool {
    crate::paths::k2_home().join("skin.db").exists()
}

// ── the switch ───────────────────────────────────────────────────────

/// Does this workspace's agent hold "Read call transcripts"?
pub fn can_read(workspace_id: &str) -> bool {
    if !store_exists() {
        return false;
    }
    crate::skin::with_skin_conn(|c| can_read_in(c, workspace_id)).unwrap_or(false)
}

pub fn can_read_in(conn: &Connection, workspace_id: &str) -> Result<bool, String> {
    conn.query_row("SELECT 1 FROM voice_transcript_readers WHERE workspace_id = ?1", params![workspace_id], |_| Ok(()))
        .optional()
        .map(|r| r.is_some())
        .map_err(|e| e.to_string())
}

/// Does any agent on this server hold the switch? (The recording notice says so.)
pub fn any_reader() -> bool {
    if !store_exists() {
        return false;
    }
    crate::skin::with_skin_conn(any_reader_in).unwrap_or(false)
}

pub fn any_reader_in(conn: &Connection) -> Result<bool, String> {
    conn.query_row("SELECT 1 FROM voice_transcript_readers LIMIT 1", [], |_| Ok(()))
        .optional()
        .map(|r| r.is_some())
        .map_err(|e| e.to_string())
}

/// The workspaces whose agents hold the switch.
pub fn readers() -> Result<Vec<String>, String> {
    if !store_exists() {
        return Ok(Vec::new());
    }
    crate::skin::with_skin_conn(|c| {
        let mut st = c.prepare("SELECT workspace_id FROM voice_transcript_readers ORDER BY workspace_id").map_err(|e| e.to_string())?;
        let rows = st.query_map([], |r| r.get::<_, String>(0)).map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
    })
}

/// Turn the switch on or off. The caller checked who may (the owner
/// raises; the owner or an Admin lowers). Off also drops the agent's room
/// subscriptions: it can't read what a note would point at.
pub fn set_reader(workspace_id: &str, on: bool, by: &str) -> Result<bool, String> {
    crate::skin::with_skin_conn(|c| set_reader_in(c, workspace_id, on, by, now()))
}

pub fn set_reader_in(conn: &Connection, workspace_id: &str, on: bool, by: &str, at: i64) -> Result<bool, String> {
    let was = can_read_in(conn, workspace_id)?;
    if on == was {
        return Ok(false);
    }
    if on {
        conn.execute(
            "INSERT INTO voice_transcript_readers (workspace_id, set_by, set_at) VALUES (?1, ?2, ?3)",
            params![workspace_id, by, at],
        )
        .map_err(|e| e.to_string())?;
    } else {
        conn.execute("DELETE FROM voice_transcript_readers WHERE workspace_id = ?1", params![workspace_id]).map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM voice_transcript_subscriptions WHERE workspace_id = ?1", params![workspace_id]).map_err(|e| e.to_string())?;
    }
    Ok(true)
}

// ── the read log ─────────────────────────────────────────────────────

/// One transcript read.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Read {
    pub id: i64,
    pub at: i64,
    /// `agent:<handle>` or `login:<user>`.
    pub reader: String,
    pub workspace_id: Option<String>,
    pub recording_id: String,
    pub lines: i64,
}

/// Log one read (every page an agent or a person reads).
pub fn log_read(reader: &str, workspace_id: Option<&str>, recording_id: &str, lines: usize) -> Result<i64, String> {
    crate::skin::with_skin_conn(|c| log_read_in(c, reader, workspace_id, recording_id, lines, now()))
}

pub fn log_read_in(conn: &Connection, reader: &str, workspace_id: Option<&str>, recording_id: &str, lines: usize, at: i64) -> Result<i64, String> {
    conn.execute(
        "INSERT INTO voice_transcript_reads (at, reader, workspace_id, recording_id, lines) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![at, reader, workspace_id, recording_id, lines as i64],
    )
    .map_err(|e| e.to_string())?;
    Ok(conn.last_insert_rowid())
}

/// The newest reads first (at most `limit`), of one recording or all.
pub fn reads(recording_id: Option<&str>, limit: u32) -> Result<Vec<Read>, String> {
    if !store_exists() {
        return Ok(Vec::new());
    }
    crate::skin::with_skin_conn(|c| reads_in(c, recording_id, limit))
}

pub fn reads_in(conn: &Connection, recording_id: Option<&str>, limit: u32) -> Result<Vec<Read>, String> {
    let mut st = conn
        .prepare(
            "SELECT id, at, reader, workspace_id, recording_id, lines FROM voice_transcript_reads
             WHERE (?1 IS NULL OR recording_id = ?1) ORDER BY id DESC LIMIT ?2",
        )
        .map_err(|e| e.to_string())?;
    let rows = st
        .query_map(params![recording_id, limit.clamp(1, 1000)], |r| {
            Ok(Read { id: r.get(0)?, at: r.get(1)?, reader: r.get(2)?, workspace_id: r.get(3)?, recording_id: r.get(4)?, lines: r.get(5)? })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

// ── "send transcripts to <agent>" per room ───────────────────────────

/// One room's subscription: notes for `workspace_id` when a recording in
/// `channel_id` has its transcript.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Subscription {
    pub channel_id: String,
    pub workspace_id: String,
    pub set_by: String,
    pub set_at: i64,
}

pub fn subscribe(channel_id: &str, workspace_id: &str, on: bool, by: &str) -> Result<bool, String> {
    crate::skin::with_skin_conn(|c| subscribe_in(c, channel_id, workspace_id, on, by, now()))
}

pub fn subscribe_in(conn: &Connection, channel_id: &str, workspace_id: &str, on: bool, by: &str, at: i64) -> Result<bool, String> {
    let n = if on {
        conn.execute(
            "INSERT OR IGNORE INTO voice_transcript_subscriptions (channel_id, workspace_id, set_by, set_at) VALUES (?1, ?2, ?3, ?4)",
            params![channel_id, workspace_id, by, at],
        )
    } else {
        conn.execute(
            "DELETE FROM voice_transcript_subscriptions WHERE channel_id = ?1 AND workspace_id = ?2",
            params![channel_id, workspace_id],
        )
    }
    .map_err(|e| e.to_string())?;
    Ok(n > 0)
}

pub fn subscriptions() -> Result<Vec<Subscription>, String> {
    if !store_exists() {
        return Ok(Vec::new());
    }
    crate::skin::with_skin_conn(subscriptions_in)
}

pub fn subscriptions_in(conn: &Connection) -> Result<Vec<Subscription>, String> {
    let mut st = conn
        .prepare("SELECT channel_id, workspace_id, set_by, set_at FROM voice_transcript_subscriptions ORDER BY channel_id, workspace_id")
        .map_err(|e| e.to_string())?;
    let rows = st
        .query_map([], |r| Ok(Subscription { channel_id: r.get(0)?, workspace_id: r.get(1)?, set_by: r.get(2)?, set_at: r.get(3)? }))
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

/// The (recording, workspace) pairs already told.
pub fn notified_in(conn: &Connection) -> Result<HashSet<(String, String)>, String> {
    let mut st = conn.prepare("SELECT recording_id, workspace_id FROM voice_transcript_notified").map_err(|e| e.to_string())?;
    let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))).map_err(|e| e.to_string())?;
    rows.collect::<Result<HashSet<_>, _>>().map_err(|e| e.to_string())
}

pub fn mark_notified_in(conn: &Connection, recording_id: &str, workspace_id: &str, at: i64) -> Result<(), String> {
    conn.execute(
        "INSERT OR IGNORE INTO voice_transcript_notified (recording_id, workspace_id, at) VALUES (?1, ?2, ?3)",
        params![recording_id, workspace_id, at],
    )
    .map(|_| ())
    .map_err(|e| e.to_string())
}

pub fn cursor_in(conn: &Connection, space_id: &str) -> Result<Option<String>, String> {
    conn.query_row("SELECT after FROM voice_transcript_cursor WHERE space_id = ?1", params![space_id], |r| r.get::<_, Option<String>>(0))
        .optional()
        .map(Option::flatten)
        .map_err(|e| e.to_string())
}

pub fn set_cursor_in(conn: &Connection, space_id: &str, after: Option<&str>) -> Result<(), String> {
    conn.execute(
        "INSERT INTO voice_transcript_cursor (space_id, after) VALUES (?1, ?2)
         ON CONFLICT(space_id) DO UPDATE SET after = excluded.after",
        params![space_id, after],
    )
    .map(|_| ())
    .map_err(|e| e.to_string())
}

// ── the K5 rooms' consent mode (Rosson 2026-11-10) ───────────────────

/// The K5 room channels K2 already set to "declining means not joining"
/// (`require_consent: true` on Sew's channel recording settings).
pub fn consent_mode_set_in(conn: &Connection) -> Result<HashSet<String>, String> {
    let mut st = conn.prepare("SELECT channel_id FROM voice_room_consent_mode").map_err(|e| e.to_string())?;
    let rows = st.query_map([], |r| r.get::<_, String>(0)).map_err(|e| e.to_string())?;
    rows.collect::<Result<HashSet<_>, _>>().map_err(|e| e.to_string())
}

pub fn consent_mode_set() -> Result<HashSet<String>, String> {
    if !store_exists() {
        return Ok(HashSet::new());
    }
    crate::skin::with_skin_conn(consent_mode_set_in)
}

pub fn note_consent_mode(channel_id: &str) -> Result<(), String> {
    crate::skin::with_skin_conn(|c| {
        c.execute("INSERT OR IGNORE INTO voice_room_consent_mode (channel_id, set_at) VALUES (?1, ?2)", params![channel_id, now()])
            .map(|_| ())
            .map_err(|e| e.to_string())
    })
}

/// Run `f` on skin.db (the daemon's notifier reads and writes in one go).
pub fn with_store<T>(f: impl FnOnce(&Connection) -> Result<T, String>) -> Result<T, String> {
    crate::skin::with_skin_conn(f)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        ensure_schema(&c).unwrap();
        c
    }

    #[test]
    fn the_switch_is_off_by_default_and_off_drops_the_agents_subscriptions() {
        let c = conn();
        assert!(!can_read_in(&c, "ws-a").unwrap(), "off by default");
        assert!(!any_reader_in(&c).unwrap());
        assert!(set_reader_in(&c, "ws-a", true, "owner", 1).unwrap());
        assert!(!set_reader_in(&c, "ws-a", true, "owner", 2).unwrap(), "already on: no change");
        assert!(can_read_in(&c, "ws-a").unwrap() && any_reader_in(&c).unwrap());
        assert!(subscribe_in(&c, "901", "ws-a", true, "owner", 3).unwrap());
        assert!(set_reader_in(&c, "ws-a", false, "admin:ada", 4).unwrap());
        assert!(!can_read_in(&c, "ws-a").unwrap());
        assert!(subscriptions_in(&c).unwrap().is_empty(), "an agent that can't read gets no notes");
    }

    #[test]
    fn every_read_is_logged_with_its_line_count() {
        let c = conn();
        log_read_in(&c, "agent:scribe", Some("ws-a"), "77", 12, 5).unwrap();
        log_read_in(&c, "login:ada", None, "78", 0, 6).unwrap();
        let all = reads_in(&c, None, 10).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!((all[1].reader.as_str(), all[1].recording_id.as_str(), all[1].lines), ("agent:scribe", "77", 12));
        assert_eq!(reads_in(&c, Some("78"), 10).unwrap().len(), 1);
    }
}
