//! A heartbeat whose WAKEUP.md is missing or can't be read waits; it is
//! never turned off for it.
//!
//! From a field report: a host problem moved a home folder away for about
//! 76 minutes. Every heartbeat that came due in that window was turned off
//! at its first fire (`wakeup_missing`), silently, and stayed off for hours
//! after the files were back. A file that is gone for a while is not a
//! reason to stop a schedule, so the tick now:
//!
//! - **Skips the fire and keeps the schedule.** The slot stays due, so the
//!   tick checks again every minute and fires one catch-up the moment the
//!   file can be read (the usual 12 h catch-up window still applies). No
//!   failure is counted and nothing is disabled.
//! - **Records each skipped fire** as one `skipped_wakeup_waiting` audit
//!   row per scheduled slot (not per tick). The rows since the last
//!   `wakeup_back` row are the open outage: when it started and how many
//!   fires it skipped. The wait state reads `wakeup_waiting` with that
//!   detail, so `k2 heartbeat list` and the roster event show it.
//! - **Tells the workspace once per outage.** One inbox note to the
//!   workspace's agent (a `wakeup_notice` row records it), and one line
//!   when the file is back (`wakeup_back`). A confirmed missing file is
//!   told at once; a read error only once it has lasted
//!   [`UNREADABLE_NOTICE_AFTER_SECS`], so a one-minute blip says nothing.
//!   While the workspace folder itself is gone the inbox can't be written
//!   (and K2 must not recreate the folder), so the note goes to the
//!   workspace's Thread instead, and its "back" line follows it there.
//!
//! "Missing" is only a confirmed absence: the read failed with NotFound,
//! the workspace folder is there and readable, and an exists check on the
//! file says no. Anything else (permission denied, an I/O error, the
//! workspace folder gone) is "can't be read". Both wait the same way; the
//! difference is only the words and how soon the note goes out.
//!
//! Heartbeats an older K2 turned off with `wakeup_missing` stay off: the
//! tick only reads enabled rows, and nothing here re-enables one.

use std::io::ErrorKind;
use std::path::Path;
use std::sync::OnceLock;

use chrono::{DateTime, Local, SecondsFormat, Utc};
use rusqlite::{params, Connection, OptionalExtension};

use crate::db::schema::AgentHeartbeat;
use crate::brand::{CLI_NAME, PRODUCT_NAME};

/// Audit decision: one scheduled fire skipped because WAKEUP.md is missing
/// or can't be read. One row per slot. The `skipped_` prefix makes every
/// client show it as a skip.
pub const DECISION_SKIPPED_WAKEUP: &str = "skipped_wakeup_waiting";
/// Audit decision: the one notice of an outage went out. Its reason
/// starts with the channel (`inbox` or `thread`).
pub const DECISION_WAKEUP_NOTICE: &str = "wakeup_notice";
/// Audit decision: WAKEUP.md can be read again. Closes the outage.
pub const DECISION_WAKEUP_BACK: &str = "wakeup_back";

/// A read error (not a confirmed missing file) is told to the workspace
/// only once it has lasted this long. Five minutes is five ticks: long
/// enough that a passing I/O blip (the fire runs a minute late) says
/// nothing, short enough that a real outage is known well before a
/// half-hourly heartbeat misses its second fire.
pub const UNREADABLE_NOTICE_AFTER_SECS: i64 = 300;

const SLOT_PREFIX: &str = "slot ";
const CHANNEL_INBOX: &str = "inbox";
const CHANNEL_THREAD: &str = "thread";

/// What reading a heartbeat's WAKEUP.md found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WakeupRead {
    /// The file's text.
    Readable(String),
    /// Confirmed gone: NotFound, the workspace folder is there and
    /// readable, and an exists check agrees.
    Missing,
    /// Anything else, with the error in words.
    Unreadable(String),
}

impl WakeupRead {
    /// The cause in plain words (empty for a readable file).
    pub fn cause(&self, rel: &str) -> String {
        match self {
            WakeupRead::Readable(_) => String::new(),
            WakeupRead::Missing => format!("WAKEUP.md not found at {rel}"),
            WakeupRead::Unreadable(e) => format!("WAKEUP.md at {rel} can't be read ({e})"),
        }
    }
}

#[cfg(test)]
thread_local! {
    /// Tests: make reads of one path fail with this error kind.
    static READ_FAULT: std::cell::RefCell<Option<(std::path::PathBuf, ErrorKind)>> =
        const { std::cell::RefCell::new(None) };
}

/// Tests: fail every read of `path` with `kind` on this thread (`None`
/// clears it).
#[cfg(test)]
pub(crate) fn set_read_fault(fault: Option<(std::path::PathBuf, ErrorKind)>) {
    READ_FAULT.with(|f| *f.borrow_mut() = fault);
}

pub(crate) fn read_file(path: &Path) -> std::io::Result<String> {
    #[cfg(test)]
    {
        let kind = READ_FAULT
            .with(|f| f.borrow().as_ref().filter(|(p, _)| p == path).map(|(_, k)| *k));
        if let Some(kind) = kind {
            return Err(std::io::Error::new(kind, "injected read fault"));
        }
    }
    std::fs::read_to_string(path)
}

/// Read the WAKEUP.md at `rel` under the workspace folder and classify a
/// failure. Never creates anything.
pub fn read_wakeup(project_path: &Path, rel: &str) -> WakeupRead {
    let path = project_path.join(rel);
    let err = match read_file(&path) {
        Ok(text) => return WakeupRead::Readable(text),
        Err(e) => e,
    };
    if err.kind() != ErrorKind::NotFound {
        return WakeupRead::Unreadable(err.to_string());
    }
    // NotFound is only "missing" when the folder it lives in is really
    // there: a workspace folder that went away (moved, a disk not
    // mounted) makes every file "not found" without anyone deleting them.
    match std::fs::metadata(project_path) {
        Ok(m) if m.is_dir() => {}
        Ok(_) => return WakeupRead::Unreadable("the workspace path is not a folder".to_string()),
        Err(e) if e.kind() == ErrorKind::NotFound => {
            return WakeupRead::Unreadable("workspace folder not found".to_string())
        }
        Err(e) => return WakeupRead::Unreadable(format!("workspace folder: {e}")),
    }
    if let Err(e) = std::fs::read_dir(project_path) {
        return WakeupRead::Unreadable(format!("workspace folder: {e}"));
    }
    if let Some(parent) = path.parent() {
        if let Err(e) = parent.try_exists() {
            return WakeupRead::Unreadable(format!("its folder: {e}"));
        }
    }
    match path.try_exists() {
        Ok(false) => WakeupRead::Missing,
        // Back between the read and the check: read it again next tick.
        Ok(true) => WakeupRead::Unreadable("the file reappeared while it was checked".to_string()),
        Err(e) => WakeupRead::Unreadable(e.to_string()),
    }
}

/// The open outage of one heartbeat: the skipped-fire rows since its last
/// `wakeup_back` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WaitingEpisode {
    /// When the first fire was skipped.
    pub since: DateTime<Utc>,
    /// Scheduled fires skipped so far.
    pub skipped: i64,
    /// The slot of the latest skipped fire (UTC RFC3339).
    pub last_slot: Option<String>,
    /// The latest cause, in words.
    pub cause: String,
    /// Where the outage's notice went (`inbox` / `thread`), if it did.
    pub noticed: Option<String>,
}

impl WaitingEpisode {
    /// The `wait_detail` while it waits. No durations, so an unchanged
    /// outage never rewrites the stored row.
    pub fn detail(&self) -> String {
        format!(
            "waiting for WAKEUP.md since {}, {} skipped: {}. {PRODUCT_NAME} retries every fire; it runs as soon as the file can be read",
            fmt_utc(self.since),
            fires(self.skipped),
            self.cause,
        )
    }
}

fn fires(n: i64) -> String {
    if n == 1 {
        "1 fire".to_string()
    } else {
        format!("{n} fires")
    }
}

fn fmt_utc(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn fmt_local(t: DateTime<Utc>) -> String {
    t.with_timezone(&Local).format("%Y-%m-%d %H:%M").to_string()
}

/// The open outage of `name`, if any.
pub fn open_waiting(conn: &Connection, project_id: &str, name: &str) -> Option<WaitingEpisode> {
    let back: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(id), 0) FROM heartbeat_fires \
             WHERE project_id = ?1 AND schedule_name = ?2 AND decision = ?3",
            params![project_id, name, DECISION_WAKEUP_BACK],
            |r| r.get(0),
        )
        .ok()?;
    let (skipped, first_id): (i64, Option<i64>) = conn
        .query_row(
            "SELECT COUNT(*), MIN(id) FROM heartbeat_fires \
             WHERE project_id = ?1 AND schedule_name = ?2 AND decision = ?3 AND id > ?4",
            params![project_id, name, DECISION_SKIPPED_WAKEUP, back],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok()?;
    let first_id = first_id.filter(|_| skipped > 0)?;
    let since: String = conn
        .query_row("SELECT fired_at FROM heartbeat_fires WHERE id = ?1", params![first_id], |r| r.get(0))
        .ok()?;
    let since = DateTime::parse_from_rfc3339(&since).ok()?.with_timezone(&Utc);
    let last_reason: Option<String> = conn
        .query_row(
            "SELECT reason FROM heartbeat_fires \
             WHERE project_id = ?1 AND schedule_name = ?2 AND decision = ?3 AND id > ?4 \
             ORDER BY id DESC LIMIT 1",
            params![project_id, name, DECISION_SKIPPED_WAKEUP, back],
            |r| r.get(0),
        )
        .optional()
        .ok()?
        .flatten();
    let (last_slot, cause) = match last_reason.as_deref().and_then(|r| r.strip_prefix(SLOT_PREFIX)) {
        Some(rest) => match rest.split_once(": ") {
            Some((slot, cause)) => (
                Some(slot.to_string()),
                cause.split_once("; ").map_or(cause, |(c, _)| c).to_string(),
            ),
            None => (None, rest.to_string()),
        },
        None => (None, last_reason.clone().unwrap_or_default()),
    };
    let noticed: Option<String> = conn
        .query_row(
            "SELECT reason FROM heartbeat_fires \
             WHERE project_id = ?1 AND schedule_name = ?2 AND decision = ?3 AND id > ?4 \
             ORDER BY id DESC LIMIT 1",
            params![project_id, name, DECISION_WAKEUP_NOTICE, back],
            |r| r.get::<_, Option<String>>(0),
        )
        .optional()
        .ok()?
        .map(|r| {
            r.unwrap_or_default()
                .split_once(':')
                .map_or(CHANNEL_INBOX.to_string(), |(c, _)| c.to_string())
        });
    Some(WaitingEpisode { since, skipped, last_slot, cause, noticed })
}

fn insert_row(
    conn: &Connection,
    project_id: &str,
    agent_name: &str,
    hb: &AgentHeartbeat,
    decision: &str,
    reason: &str,
    at: DateTime<Utc>,
) {
    // Same row shape as `HeartbeatFire::insert_with_schedule`, with the
    // caller's clock so an outage's start and its rows agree.
    if let Err(e) = conn.execute(
        "INSERT INTO heartbeat_fires \
         (project_id, agent_name, fired_at, mode, decision, reason, schedule_name) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            project_id,
            agent_name,
            at.with_timezone(&Local).to_rfc3339(),
            hb.frequency,
            decision,
            reason,
            hb.name
        ],
    ) {
        crate::log_debug!("[heartbeat-tick] {} {decision} audit: {e}", hb.name);
    }
}

// ── Notes to the Thread (daemon) ──────────────────────────────────────

/// `code`: the note's thread-system-notice-v1 code ([`NOTE_WAITING`] /
/// [`NOTE_BACK`]).
type ThreadSink = fn(project_path: &str, code: &str, text: &str) -> Result<(), String>;
/// Thread note code: a heartbeat is waiting for its WAKEUP.md.
pub const NOTE_WAITING: &str = "heartbeat.wakeup_waiting";
/// Thread note code: its WAKEUP.md can be read again.
pub const NOTE_BACK: &str = "heartbeat.wakeup_back";
static THREAD_SINK: OnceLock<ThreadSink> = OnceLock::new();

/// The daemon registers how a note reaches the workspace's Thread (the
/// Thread store is the daemon's). Used only while the workspace folder is
/// gone, so the inbox can't be written. First call wins.
pub fn set_thread_sink(sink: ThreadSink) {
    let _ = THREAD_SINK.set(sink);
}

/// Deliver one note: the workspace's inbox while its folder is there,
/// else its Thread. `thread_first` sends a "back" line where the outage's
/// notice went. Returns the channel it went to.
fn deliver(project_path: &str, code: &str, title: &str, body: &str, thread_first: bool) -> Result<&'static str, String> {
    let dir = Path::new(project_path);
    // Never `inbox::compose` into a folder that went away: it would
    // create the folder again where the real one is about to be restored.
    if !thread_first && dir.is_dir() {
        match crate::inbox::compose(dir, title, body, Some("high"), Some("heartbeat"), Some("k2")) {
            Ok(_) => return Ok(CHANNEL_INBOX),
            Err(e) => crate::log_debug!("[heartbeat-tick] inbox note for {project_path}: {e}"),
        }
    }
    let sink = THREAD_SINK.get().ok_or_else(|| "no Thread sink registered".to_string())?;
    sink(project_path, code, &format!("{title}\n\n{body}"))?;
    Ok(CHANNEL_THREAD)
}

fn notice_text(hb: &AgentHeartbeat, ep: &WaitingEpisode) -> (String, String) {
    let title = format!("Heartbeat {} is waiting for its WAKEUP.md", hb.name);
    let body = format!(
        "{PRODUCT_NAME} skipped heartbeat '{name}' at {at}: {cause}.\n\n\
         Nothing is turned off. The heartbeat stays on and {PRODUCT_NAME} keeps retrying: \
         it checks again every minute while a fire is due, and it runs as soon \
         as the file can be read. You get one more note when it can be read again.\n\n\
         - If the file was deleted by mistake, put it back at {rel}.\n\
         - If you meant to stop this heartbeat, turn it off: {CLI_NAME} heartbeat disable {name}\n\
         - See where it stands: {CLI_NAME} heartbeat list",
        name = hb.name,
        at = fmt_local(ep.since),
        cause = ep.cause,
        rel = hb.wakeup_path,
    );
    (title, body)
}

fn back_text(hb: &AgentHeartbeat, ep: &WaitingEpisode) -> (String, String) {
    let title = format!("Heartbeat {} is back to normal", hb.name);
    let body = format!(
        "WAKEUP.md for heartbeat '{name}' is back. It was missing or unreadable from {since} \
         and {skipped} skipped. It runs now, and on its schedule after that.",
        name = hb.name,
        since = fmt_local(ep.since),
        skipped = fires(ep.skipped),
    );
    (title, body)
}

/// The tick's check for a due heartbeat fire at `slot`. Returns `true`
/// when the fire is skipped (WAKEUP.md missing or unreadable), `false`
/// when it may go ahead. Never disables the row and never counts a
/// failure. Holds no lock of its own; the caller's connection is used.
#[allow(clippy::too_many_arguments)]
pub fn skip_due_fire(
    conn: &Connection,
    project_path: &str,
    project_id: &str,
    agent_name: &str,
    hb: &AgentHeartbeat,
    slot: DateTime<Local>,
    now: DateTime<Utc>,
) -> bool {
    let read = read_wakeup(Path::new(project_path), &hb.wakeup_path);
    let episode = open_waiting(conn, project_id, &hb.name);
    if let WakeupRead::Readable(_) = read {
        if let Some(ep) = episode {
            recovered(conn, project_path, project_id, agent_name, hb, &ep, now);
        }
        return false;
    }

    let cause = read.cause(&hb.wakeup_path);
    let slot = fmt_utc(slot.with_timezone(&Utc));
    if episode.as_ref().and_then(|e| e.last_slot.as_deref()) != Some(slot.as_str()) {
        insert_row(
            conn,
            project_id,
            agent_name,
            hb,
            DECISION_SKIPPED_WAKEUP,
            &format!(
                "{SLOT_PREFIX}{slot}: {cause}; skipped, the heartbeat stays on and {PRODUCT_NAME} retries"
            ),
            now,
        );
        crate::log_debug!("[heartbeat-tick] {} skipped the {slot} fire: {cause}", hb.name);
    }
    let Some(ep) = open_waiting(conn, project_id, &hb.name) else {
        return true;
    };
    if ep.noticed.is_none() {
        let tell = read == WakeupRead::Missing
            || (now - ep.since).num_seconds() >= UNREADABLE_NOTICE_AFTER_SECS;
        if tell {
            let (title, body) = notice_text(hb, &ep);
            match deliver(project_path, NOTE_WAITING, &title, &body, false) {
                Ok(channel) => insert_row(
                    conn,
                    project_id,
                    agent_name,
                    hb,
                    DECISION_WAKEUP_NOTICE,
                    &format!("{channel}: told the workspace it is waiting for WAKEUP.md"),
                    now,
                ),
                // Not recorded, so the next tick tries again.
                Err(e) => crate::log_debug!("[heartbeat-tick] {} waiting notice: {e}", hb.name),
            }
        }
    }
    true
}

/// WAKEUP.md can be read again: close the outage, and send the one "back"
/// line if the outage was told.
fn recovered(
    conn: &Connection,
    project_path: &str,
    project_id: &str,
    agent_name: &str,
    hb: &AgentHeartbeat,
    ep: &WaitingEpisode,
    now: DateTime<Utc>,
) {
    insert_row(
        conn,
        project_id,
        agent_name,
        hb,
        DECISION_WAKEUP_BACK,
        &format!(
            "WAKEUP.md can be read again after {} skipped since {}",
            fires(ep.skipped),
            fmt_utc(ep.since)
        ),
        now,
    );
    crate::log_debug!("[heartbeat-tick] {} WAKEUP.md is back after {}", hb.name, fires(ep.skipped));
    if ep.noticed.is_some() {
        let (title, body) = back_text(hb, ep);
        let thread_first = ep.noticed.as_deref() == Some(CHANNEL_THREAD);
        if let Err(e) = deliver(project_path, NOTE_BACK, &title, &body, thread_first) {
            crate::log_debug!("[heartbeat-tick] {} back note: {e}", hb.name);
        }
    }
}

/// The text a wait state shows for a heartbeat whose WAKEUP.md is missing
/// or unreadable but has not skipped a fire yet.
pub fn not_yet_detail(read: &WakeupRead, rel: &str) -> String {
    format!(
        "{}. Its next fire is skipped until the file can be read; the heartbeat stays on",
        read.cause(rel)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(label: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "k2-wread-{label}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(d.join("hb")).unwrap();
        d
    }

    #[test]
    fn a_present_file_reads() {
        let d = tmp("ok");
        std::fs::write(d.join("hb/WAKEUP.md"), "go\n").unwrap();
        assert_eq!(read_wakeup(&d, "hb/WAKEUP.md"), WakeupRead::Readable("go\n".to_string()));
    }

    #[test]
    fn not_found_in_a_readable_folder_is_missing() {
        let d = tmp("gone");
        assert_eq!(read_wakeup(&d, "hb/WAKEUP.md"), WakeupRead::Missing);
        // Its own folder deleted too: still a confirmed absence.
        std::fs::remove_dir(d.join("hb")).unwrap();
        assert_eq!(read_wakeup(&d, "hb/WAKEUP.md"), WakeupRead::Missing);
    }

    #[test]
    fn not_found_with_the_workspace_folder_gone_is_unreadable() {
        let d = tmp("ws-gone");
        std::fs::remove_dir_all(&d).unwrap();
        match read_wakeup(&d, "hb/WAKEUP.md") {
            WakeupRead::Unreadable(e) => assert_eq!(e, "workspace folder not found"),
            other => panic!("a missing workspace folder is not a missing file: {other:?}"),
        }
        assert!(!d.exists(), "reading never creates the folder");
    }

    #[test]
    fn other_read_errors_are_unreadable_not_missing() {
        let d = tmp("fault");
        let path = d.join("hb/WAKEUP.md");
        std::fs::write(&path, "go\n").unwrap();
        for kind in [ErrorKind::PermissionDenied, ErrorKind::Other, ErrorKind::Interrupted] {
            set_read_fault(Some((path.clone(), kind)));
            let got = read_wakeup(&d, "hb/WAKEUP.md");
            set_read_fault(None);
            match got {
                WakeupRead::Unreadable(e) => assert!(e.contains("injected read fault"), "{kind:?}: {e}"),
                other => panic!("{kind:?} must be unreadable, got {other:?}"),
            }
        }
    }
}
