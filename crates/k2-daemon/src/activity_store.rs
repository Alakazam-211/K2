//! The daemon's activity store (prd-daemon-activity-and-thread-working-v1
//! S2: DA1–DA4, DA19–DA26, DA29–DA32).
//!
//! One [`Row`] per live v2 session, keyed by the v2 session id (DA2): the
//! hook's `X-K2-Pane`. A row is created in `v2_session_map::register` and
//! removed in `unregister`; nothing else creates or removes rows. The row
//! logic is pure (`k2_core::activity`); this module feeds it and performs
//! the side effects each [`Change`] asks for:
//! - `workspace_sessions.status` (DA32): the store only *releases* the
//!   session lock on turn-end evidence, decay or exit, re-claims it on
//!   confirmed work, and never writes on an unconfirmed row;
//! - the legacy lifecycle outputs (RL5): `agent:lifecycle` and
//!   `agent_status_changed`, derived from the row's display;
//! - the Active touch on lead → working (Q14, 5-minute debounce).
//!
//! Inputs: owner envelopes and legacy posts from [`crate::hook_ingest`]
//! (the only status writer for hook events now), the title observer
//! (`session_activity`), client keystrokes (`sessions_grid_ws`,
//! `/cli/terminal/write`), and a timer task that fires row deadlines
//! (owed lease, key settle, 30-minute decay) plus the 10 s liveness sweep
//! (dead PTYs, dead hook owners).
//!
//! The store is in memory by design (§8): after a restart no PTY is live,
//! so rows start fresh and unconfirmed (DA30), and boot releases every
//! stale lock ([`boot_reset_status`]).
//!
//! Seams: [`subscribe`] carries every row change (S4 builds
//! `activity_changed` + `seq` and the snapshot route on it; Keep awake
//! reads it today), [`apply`] takes S3's transcript and screen evidence
//! ([`crate::activity_transcript`] follows each session's transcript
//! from register to unregister), and [`RowEvent::turn_ended`] is S6's
//! turn end. S6 (`thread_activity`) reads rows with [`with_row`] and is
//! handed every owner envelope after the row has applied it.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::Duration;

use parking_lot::Mutex;
use tokio::sync::broadcast;

use k2_core::activity::{fold, Change, Evidence, KeyInput, Row, RowFacts, TitleSignal, TurnEnded};
use k2_core::log_debug;
use k2_core::session::SessionId;

use crate::hook_ingest::IngestEvent;

/// DA14-6 / §5.4: how often dead PTYs and dead hook owners are swept.
pub const LIVENESS_SWEEP: Duration = Duration::from_secs(10);

/// Q14 / RL12: one Active touch per workspace per this window.
pub const ACTIVE_TOUCH_DEBOUNCE_MS: i64 = 5 * 60 * 1000;

/// Keep awake (A32): an `unverifiable` row holds this long after its
/// last evidence (`power::keep_awake::WORKING_STALE`).
pub const KEEP_AWAKE_STALE_MS: i64 = 2 * 3600 * 1000;

const BUS_CAP: usize = 1024;

/// One row change, for in-process subscribers (Keep awake today; S4's
/// `activity_changed` and snapshot).
// `agent_name`, `workspace_path` and `turn_ended` are read by S4 (rollup,
// event) and S6 (Thread turn ends).
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct RowEvent {
    pub session_id: String,
    pub agent_name: String,
    pub workspace_path: Option<String>,
    /// §7.2 row JSON; `None` when the row was removed.
    pub row: Option<serde_json::Value>,
    pub turn_ended: Option<TurnEnded>,
    /// Q10 / A32: this row holds Keep awake right now.
    pub holds_awake: bool,
    /// The row's last evidence (unix ms), for Keep awake's 2 h clock.
    pub evidence_at: Option<i64>,
}

struct Entry {
    row: Row,
    /// The project id when this is the workspace's pinned chat
    /// (`agent_name == project_id`): its `workspace_sessions` row may be
    /// keyed `agent-chat:<pid>` (A14 d).
    pinned_project: Option<String>,
}

#[derive(Default)]
struct Store {
    rows: HashMap<String, Entry>,
    /// project id → last daemon Active touch (ms).
    touched: HashMap<String, i64>,
}

fn store() -> &'static Mutex<Store> {
    static S: OnceLock<Mutex<Store>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(Store::default()))
}

fn bus() -> &'static broadcast::Sender<RowEvent> {
    static B: OnceLock<broadcast::Sender<RowEvent>> = OnceLock::new();
    B.get_or_init(|| broadcast::channel(BUS_CAP).0)
}

/// The timer thread's wake-up: set by every input, waited on with the
/// earliest deadline as the timeout.
fn wake_cell() -> &'static (std::sync::Mutex<bool>, std::sync::Condvar) {
    static W: OnceLock<(std::sync::Mutex<bool>, std::sync::Condvar)> = OnceLock::new();
    W.get_or_init(|| (std::sync::Mutex::new(false), std::sync::Condvar::new()))
}

fn wake() {
    let (flag, cv) = wake_cell();
    if let Ok(mut woken) = flag.lock() {
        *woken = true;
        cv.notify_one();
    }
}

fn wait_for_wake(timeout: Duration) {
    let (flag, cv) = wake_cell();
    let Ok(guard) = flag.lock() else { return };
    let (mut guard, _) = match cv.wait_timeout_while(guard, timeout, |woken| !*woken) {
        Ok(r) => r,
        Err(_) => return,
    };
    *guard = false;
}

/// Every row change and removal.
pub fn subscribe() -> broadcast::Receiver<RowEvent> {
    bus().subscribe()
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// The work a change asks for, gathered under the store lock and run
/// after it is released.
struct Effects {
    session_id: String,
    pinned_project: Option<String>,
    project_id: Option<String>,
    workspace_path: Option<String>,
    change: Change,
    event: Option<RowEvent>,
    touch: bool,
}

fn effects_for(st: &mut Store, sid: &str, change: Change, now: i64) -> Option<Effects> {
    let entry = st.rows.get(sid)?;
    let row = &entry.row;
    let event = (change.changed || change.turn_ended.is_some()).then(|| RowEvent {
        session_id: sid.to_string(),
        agent_name: row.facts.agent_name.clone(),
        workspace_path: row.facts.workspace_path.clone(),
        row: Some(row.to_json()),
        turn_ended: change.turn_ended,
        holds_awake: fold::holds_awake(row, now, KEEP_AWAKE_STALE_MS),
        evidence_at: row.evidence_at,
    });
    let project_id = row.facts.project_id.clone();
    let mut touch = false;
    if change.lead_started_working {
        if let Some(pid) = project_id.as_deref() {
            let last = st.touched.get(pid).copied();
            if last.map_or(true, |t| now - t >= ACTIVE_TOUCH_DEBOUNCE_MS) {
                st.touched.insert(pid.to_string(), now);
                touch = true;
            }
        }
    }
    let entry = st.rows.get(sid)?;
    Some(Effects {
        session_id: sid.to_string(),
        pinned_project: entry.pinned_project.clone(),
        project_id,
        workspace_path: entry.row.facts.workspace_path.clone(),
        change,
        event,
        touch,
    })
}

fn run_effects(fx: Effects) {
    if let Some(status) = fx.change.status_write {
        write_status(&fx.session_id, fx.pinned_project.as_deref(), status);
    }
    if let Some(word) = fx.change.compat {
        k2_core::agent_hooks::emit_lifecycle(&fx.session_id, word, fx.workspace_path.as_deref());
    }
    if fx.touch {
        if let Some(pid) = fx.project_id.as_deref() {
            // Q14: a confirmed turn keeps its workspace Active, headless
            // or not (the renderer's activity touch is retired in S5).
            if let Err(e) = k2_core::projects_ops::projects_touch_interaction(pid) {
                log_debug!("[activity] active touch failed for {pid}: {e}");
            }
        }
    }
    if let Some(ev) = fx.event {
        let _ = bus().send(ev);
    }
}

/// DA32: write the session lock for the `workspace_sessions` row this
/// session backs: `terminal_id` or `active_terminal_id` is the session,
/// or (pinned chat) the workspace's own row. Only when it differs.
fn write_status(session_id: &str, pinned_project: Option<&str>, status: &str) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let res = conn.execute(
        "UPDATE workspace_sessions SET status = ?1, last_activity_at = unixepoch() \
         WHERE status != ?1 AND (terminal_id = ?2 OR active_terminal_id = ?2 \
           OR (?3 IS NOT NULL AND project_id = ?3))",
        rusqlite::params![status, session_id, pinned_project],
    );
    match res {
        Ok(n) if n > 0 => log_debug!("[activity] status {status} for session {session_id} ({n} row)"),
        Ok(_) => {}
        Err(e) => log_debug!("[activity] status write failed for {session_id}: {e}"),
    }
}

/// DA32 (d) + unregister: release the lock and un-surface the row this
/// session backed. Runs BEFORE `active_terminal_id` is nulled, and also
/// matches the pinned chat's `agent-chat:<pid>` row (A14 d).
pub fn release_lock_on_unregister(conn: &rusqlite::Connection, session_id: &str, agent_name: &str) {
    let pinned = pinned_project_for(conn, agent_name);
    let _ = conn.execute(
        "UPDATE workspace_sessions SET surfaced = 0, status = 'sleeping' \
         WHERE terminal_id = ?1 OR active_terminal_id = ?1 \
           OR (?2 IS NOT NULL AND project_id = ?2 AND terminal_id = 'agent-chat:' || ?2)",
        rusqlite::params![session_id, pinned],
    );
}

/// `agent_name` when it is a registered project id (the pinned chat key).
fn pinned_project_for(conn: &rusqlite::Connection, agent_name: &str) -> Option<String> {
    conn.query_row(
        "SELECT id FROM projects WHERE id = ?1",
        rusqlite::params![agent_name],
        |r| r.get::<_, String>(0),
    )
    .ok()
}

/// DA30: after a restart no in-process PTY is live, so every `running` /
/// `permission` lock and every `active_terminal_id` points at a corpse.
/// Runs at boot before any heartbeat tick. Returns rows released.
///
/// k2-ptyd S3 (PRD prd-k2-ptyd-v1 P35): a session that survived the
/// restart in k2-ptyd (`keep`: B0's running session ids) is adopted under
/// the same id, so a row whose `active_terminal_id` is one of them keeps
/// its status and its stamp. With an empty `keep` this is exactly
/// [`boot_reset_status`].
pub fn boot_reset_status_keeping(conn: &rusqlite::Connection, keep: &std::collections::HashSet<String>) -> usize {
    if keep.is_empty() {
        return boot_reset_status(conn);
    }
    let rows: Vec<(String, Option<String>, String)> = conn
        .prepare("SELECT project_id, active_terminal_id, status FROM workspace_sessions")
        .and_then(|mut st| {
            st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, String>(2)?)))?
                .collect::<Result<Vec<_>, _>>()
        })
        .unwrap_or_default();
    let mut released = 0;
    for (project_id, terminal, status) in rows {
        if terminal.as_deref().is_some_and(|t| keep.contains(t)) {
            continue;
        }
        if status == "running" || status == "permission" {
            released += conn
                .execute("UPDATE workspace_sessions SET status = 'sleeping' WHERE project_id = ?1", [&project_id])
                .unwrap_or(0);
        }
        if terminal.is_some() {
            let _ = conn.execute("UPDATE workspace_sessions SET active_terminal_id = NULL WHERE project_id = ?1", [&project_id]);
        }
    }
    released
}

/// DA30, before k2-ptyd: no PTY survives, release every lock and stamp.
pub fn boot_reset_status(conn: &rusqlite::Connection) -> usize {
    let released = conn
        .execute(
            "UPDATE workspace_sessions SET status = 'sleeping' WHERE status IN ('running','permission')",
            [],
        )
        .unwrap_or(0);
    let _ = conn.execute(
        "UPDATE workspace_sessions SET active_terminal_id = NULL WHERE active_terminal_id IS NOT NULL",
        [],
    );
    released
}

/// The harness word for a spawn command (`claude`, `codex`, …, or the
/// command's basename; `shell` for a login shell).
pub fn harness_for(program: Option<&str>) -> String {
    let Some(program) = program.map(str::trim).filter(|p| !p.is_empty()) else {
        return "shell".to_string();
    };
    if let Some(p) = k2_core::workspace::provider_resume::provider_resume_for_command(program) {
        return p.provider.to_string();
    }
    std::path::Path::new(program)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| program.to_string())
}

/// The workspace (project id, path) that owns `cwd`: the longest
/// registered project path that is a path-boundary prefix of it.
pub(crate) fn project_for_cwd(cwd: &str) -> Option<(String, String)> {
    if cwd.is_empty() {
        return None;
    }
    project_in(cwd, &registered_projects())
}

/// Every registered project as `(id, path)` (the audit sentinels left
/// out): one read for callers that resolve many cwds.
pub(crate) fn registered_projects() -> Vec<(String, String)> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let Ok(mut stmt) = conn.prepare("SELECT id, path FROM projects WHERE id NOT IN ('_orphan', '_broadcast')")
    else {
        return Vec::new();
    };
    let Ok(rows) = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))) else {
        return Vec::new();
    };
    rows.filter_map(Result::ok).collect()
}

/// [`project_for_cwd`] against an already-read project list.
pub(crate) fn project_in(cwd: &str, projects: &[(String, String)]) -> Option<(String, String)> {
    if cwd.is_empty() {
        return None;
    }
    let paths: Vec<String> = projects.iter().map(|(_, p)| p.clone()).collect();
    let idx = crate::session_events::longest_registered_project_index(cwd, &paths)?;
    projects.get(idx).cloned()
}

/// What the store needs to know about a session at registration.
#[derive(Debug, Clone)]
pub struct SessionFacts {
    pub session_id: String,
    pub agent_name: String,
    pub cwd: Option<String>,
    pub program: Option<String>,
}

/// DA2: create the row for a newly registered v2 session (idle,
/// unconfirmed, DA30). Replaces any row under the same id.
pub fn register(facts: SessionFacts) {
    let project = facts.cwd.as_deref().and_then(project_for_cwd);
    let pinned_project = project
        .as_ref()
        .filter(|(pid, _)| *pid == facts.agent_name)
        .map(|(pid, _)| pid.clone());
    let workspace_path = project.as_ref().map(|(_, p)| p.clone()).or(facts.cwd.clone());
    let now = now_ms();
    let row = Row::new(
        RowFacts {
            session_id: facts.session_id.clone(),
            agent_name: facts.agent_name,
            project_id: project.map(|(pid, _)| pid),
            workspace_path,
            harness: harness_for(facts.program.as_deref()),
        },
        now,
    );
    // S3: follow the session's transcript (or its screen) for evidence.
    crate::activity_transcript::track(
        crate::activity_transcript::TrackFacts {
            session_id: facts.session_id.clone(),
            agent_name: row.facts.agent_name.clone(),
            harness: row.facts.harness.clone(),
            cwd: facts.cwd.clone(),
            project_id: row.facts.project_id.clone(),
        },
        now,
    );
    store().lock().rows.insert(facts.session_id, Entry { row, pinned_project });
}

/// DA2: the session went away. Removes the row and, when an older client
/// last heard `start`/`permission`, sends the compat `stop` (RL5). The
/// lock itself is released by [`release_lock_on_unregister`].
pub fn unregister(session_id: &str) {
    crate::activity_transcript::untrack(session_id);
    let removed = store().lock().rows.remove(session_id);
    let Some(entry) = removed else { return };
    let row = entry.row;
    if matches!(row.compat_heard(), Some("start" | "permission")) {
        k2_core::agent_hooks::emit_lifecycle(session_id, "stop", row.facts.workspace_path.as_deref());
    }
    let _ = bus().send(RowEvent {
        session_id: session_id.to_string(),
        agent_name: row.facts.agent_name.clone(),
        workspace_path: row.facts.workspace_path.clone(),
        row: None,
        turn_ended: None,
        holds_awake: false,
        evidence_at: row.evidence_at,
    });
    wake();
}

// ── k2-ptyd P37: a row carried across a daemon restart ─────────────────
//
// A restart that keeps agents (§7.6 `KeepAgents`, hand-off quiesce) writes
// each ptyd session's row to `<K2 home>/run/activity/<sid>.json` (0600,
// temp + rename) just before the session is detached. The next daemon's
// B1 registers the adopted session (a fresh, unconfirmed row, DA30) and
// then seeds it from that file when it is under [`CARRIED_ROW_MAX_AGE_MS`]
// old, so a turn that was running is still `working` after the restart.
// Older files, and every crash (nothing was written), start unconfirmed.
// The file holds the row only (states, counts, times, tool names): no env
// values, tokens or screen text (P72).

/// P37: a carried row seeds the adopted row only when it is this fresh.
pub const CARRIED_ROW_MAX_AGE_MS: i64 = 10 * 60 * 1000;

/// `<K2 home>/run/activity` (P88: from the K2 home).
pub fn carried_rows_dir() -> std::path::PathBuf {
    k2_core::paths::k2_home().join("run").join("activity")
}

fn carried_row_path(session_id: &str) -> Option<std::path::PathBuf> {
    // A session id is a UUID; never let one name a path outside the dir.
    let ok = !session_id.is_empty() && session_id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
    ok.then(|| carried_rows_dir().join(format!("{session_id}.json")))
}

#[derive(serde::Serialize, serde::Deserialize)]
struct CarriedRow {
    v: u32,
    saved_at: i64,
    row: Row,
}

/// P37: write this session's row for the next daemon. `Ok(false)` when
/// the session has no row.
pub fn save_for_restart(session_id: &str) -> std::io::Result<bool> {
    let Some(path) = carried_row_path(session_id) else {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "not a session id"));
    };
    let Some(row) = store().lock().rows.get(session_id).map(|e| e.row.clone()) else {
        return Ok(false);
    };
    let body = serde_json::to_vec(&CarriedRow { v: 1, saved_at: now_ms(), row }).map_err(std::io::Error::other)?;
    // Security review F8: a private dir checked without following a
    // symlink, an unpredictable temp name, create_new + O_NOFOLLOW, rename.
    #[cfg(unix)]
    k2_core::terminal::ptyd::snapshot::write_private_atomic(&path, &body)?;
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(carried_rows_dir())?;
        std::fs::write(&path, &body)?;
    }
    Ok(true)
}

/// Pure part of [`restore_after_adoption`]: the carried row when `raw`
/// parses and is fresh at `now`.
fn carried_row_if_fresh(raw: &[u8], now: i64) -> Option<Row> {
    let c: CarriedRow = serde_json::from_slice(raw).ok()?;
    let age = now - c.saved_at;
    (c.v == 1 && (0..=CARRIED_ROW_MAX_AGE_MS).contains(&age)).then_some(c.row)
}

/// P37: B1 registered the adopted session; seed its row from the file the
/// previous daemon wrote (always deleted). The new registration's facts
/// win (workspace, harness); the rest — lead state, children, evidence,
/// confirmation — comes back. Returns whether the row was seeded.
pub fn restore_after_adoption(session_id: &str) -> bool {
    let Some(path) = carried_row_path(session_id) else { return false };
    // Security review F7: a regular file (not a symlink) of at most
    // CARRIED_ROW_MAX_BYTES, read through O_NOFOLLOW; it is removed either
    // way, and anything else is just logged (the row starts unconfirmed).
    let Ok(md) = std::fs::symlink_metadata(&path) else { return false };
    let raw = if md.is_file() && md.len() <= CARRIED_ROW_MAX_BYTES {
        read_capped(&path, CARRIED_ROW_MAX_BYTES)
    } else {
        None
    };
    let _ = std::fs::remove_file(&path);
    let Some(raw) = raw else {
        log_debug!("[activity] carried row for {session_id} refused (not a regular file under {CARRIED_ROW_MAX_BYTES} bytes); removed");
        return false;
    };
    restore_from(session_id, &raw, now_ms())
}

/// F7: a carried row is a few KiB; anything over this is refused.
pub const CARRIED_ROW_MAX_BYTES: u64 = 1 << 20;

fn read_capped(path: &std::path::Path, max: u64) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut opts = std::fs::OpenOptions::new();
    opts.read(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::custom_flags(&mut opts, libc::O_NOFOLLOW);
    let mut buf = Vec::new();
    opts.open(path).ok()?.take(max + 1).read_to_end(&mut buf).ok()?;
    (buf.len() as u64 <= max).then_some(buf)
}

/// [`restore_after_adoption`] without the file: seed the registered row
/// of `session_id` from `raw` when it is fresh at `now`.
fn restore_from(session_id: &str, raw: &[u8], now: i64) -> bool {
    let Some(mut carried) = carried_row_if_fresh(raw, now) else {
        log_debug!("[activity] carried row for {session_id} is stale or unreadable; starting unconfirmed");
        return false;
    };
    let event = {
        let mut st = store().lock();
        let Some(entry) = st.rows.get_mut(session_id) else { return false };
        carried.facts = entry.row.facts.clone();
        entry.row = carried;
        let row = &entry.row;
        RowEvent {
            session_id: session_id.to_string(),
            agent_name: row.facts.agent_name.clone(),
            workspace_path: row.facts.workspace_path.clone(),
            row: Some(row.to_json()),
            turn_ended: None,
            holds_awake: fold::holds_awake(row, now, KEEP_AWAKE_STALE_MS),
            evidence_at: row.evidence_at,
        }
    };
    let _ = bus().send(event);
    wake();
    true
}

/// P37: drop carried rows no adopted session will claim (B1, with the set
/// B0 found running). Returns how many were removed.
pub fn sweep_carried_rows(keep: &std::collections::HashSet<String>) -> usize {
    let Ok(rd) = std::fs::read_dir(carried_rows_dir()) else { return 0 };
    let mut n = 0;
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let sid = name.strip_suffix(".json").unwrap_or(&name);
        if !keep.contains(sid) && std::fs::remove_file(e.path()).is_ok() {
            n += 1;
        }
    }
    n
}

/// Apply one input to a session's row at `now` and run its effects.
/// Unknown sessions are ignored (the row is created at registration).
pub fn apply_at(session_id: &str, ev: Evidence<'_>, now: i64) {
    let fx = {
        let mut st = store().lock();
        let Some(entry) = st.rows.get_mut(session_id) else { return };
        let change = entry.row.apply(ev, now);
        effects_for(&mut st, session_id, change, now)
    };
    if let Some(fx) = fx {
        run_effects(fx);
    }
    wake();
}

/// [`apply_at`] now.
pub fn apply(session_id: &str, ev: Evidence<'_>) {
    apply_at(session_id, ev, now_ms());
}

/// The title observer's word for a session (DA28).
pub fn apply_title(session_id: &str, signal: TitleSignal) {
    // A title change means the agent is doing something: a parked
    // transcript follower reads again (S3).
    crate::activity_transcript::wake(session_id);
    apply(session_id, Evidence::Title(signal));
}

/// DA26 / A15: a client keystroke on its way to the PTY (grid WS input,
/// `/cli/terminal/write`). Only a lone Esc or Ctrl-C matters; injected
/// text (msg, Thread, heartbeat) never comes through here.
pub fn note_client_input(session_id: &str, input: &[u8]) {
    // Someone typed: a parked transcript follower reads again (S3).
    crate::activity_transcript::wake(session_id);
    if let Some(key) = KeyInput::classify(input) {
        apply(session_id, Evidence::Key(key));
    }
}

/// Fire every row's due timers at `now`.
pub fn tick_all(now: i64) {
    let effects: Vec<Effects> = {
        let mut st = store().lock();
        let due: Vec<String> = st
            .rows
            .iter()
            .filter(|(_, e)| e.row.next_deadline().is_some_and(|t| t <= now))
            .map(|(k, _)| k.clone())
            .collect();
        due.into_iter()
            .filter_map(|sid| {
                let change = st.rows.get_mut(&sid)?.row.tick(now);
                effects_for(&mut st, &sid, change, now)
            })
            .collect()
    };
    for fx in effects {
        run_effects(fx);
    }
}

/// The earliest row deadline.
fn next_deadline() -> Option<i64> {
    store().lock().rows.values().filter_map(|e| e.row.next_deadline()).min()
}

/// The 10 s liveness sweep: a row whose PTY child is gone gets
/// `pty_exited`; dead hook owners are released through
/// [`crate::hook_ingest::sweep_live`] (→ `agent_exited`).
pub fn sweep_liveness() {
    let ids: Vec<String> = store().lock().rows.keys().cloned().collect();
    for sid in ids {
        let alive = SessionId::parse(&sid)
            .and_then(|id| crate::v2_session_map::lookup_by_session_id(&id))
            .is_some_and(|s| s.is_child_alive());
        if !alive {
            apply(&sid, Evidence::PtyExited);
        }
    }
    crate::hook_ingest::sweep_live();
}

/// One ingest event from the hook plane.
pub fn apply_ingest(ev: &IngestEvent) {
    match ev {
        IngestEvent::Envelope { envelope, received_at_ms, claimed_now, conversation_changed } => {
            // A18: the owner's conversation id is the transcript to follow;
            // a claim retries an unresolved one at once.
            if let Some(conv) = conversation_changed.as_deref() {
                crate::activity_transcript::note_conversation(&envelope.pane, conv);
            }
            if *claimed_now {
                crate::activity_transcript::note_claim(&envelope.pane);
            }
            apply_at(&envelope.pane, Evidence::Hook(envelope), *received_at_ms);
            // S6: the Thread turn tracker reads the step (tool line,
            // stamp binding) after the row has taken the envelope, so the
            // row it reads already reflects it.
            crate::thread_activity::on_envelope(envelope, *received_at_ms);
        }
        IngestEvent::OwnerReleased { pane, at_ms } => apply_at(pane, Evidence::OwnerReleased, *at_ms),
        IngestEvent::Legacy { pane, raw_event, received_at_ms } => {
            if let Some(bucket) = k2_core::agent_hooks::map_event_type(raw_event) {
                apply_at(pane, Evidence::LegacyHook(bucket), *received_at_ms);
            }
        }
    }
}

/// What the transcript follower needs to pick its cadence (S3).
#[derive(Debug, Clone, Copy)]
pub struct FollowView {
    /// Read fast: the lead is mid-turn or a record is awaited.
    pub hot: bool,
    pub display_idle: bool,
    pub had_hook: bool,
    pub had_transcript: bool,
}

/// The follower's view of one row, or `None` when it is gone.
pub fn follow_view(session_id: &str) -> Option<FollowView> {
    let st = store().lock();
    let row = &st.rows.get(session_id)?.row;
    Some(FollowView {
        hot: row.transcript_hot(),
        display_idle: row.display == k2_core::activity::Display::Idle,
        had_hook: row.had_hook(),
        had_transcript: row.had_transcript(),
    })
}

/// S3: a transcript path resolved (or was lost) for this session (A16:
/// a Ctrl-C then waits for the interrupt record, not the 500 ms settle).
pub fn set_transcript_resolvable(session_id: &str, yes: bool) {
    if let Some(entry) = store().lock().rows.get_mut(session_id) {
        entry.row.set_transcript_resolvable(yes);
    }
}

/// The §7.2 JSON of one row (tests; S4's snapshot).
pub fn row_json(session_id: &str) -> Option<serde_json::Value> {
    store().lock().rows.get(session_id).map(|e| e.row.to_json())
}

/// Read one live row (S6: the Thread turn tracker's view of its session).
pub fn with_row<T>(session_id: &str, f: impl FnOnce(&Row) -> T) -> Option<T> {
    store().lock().rows.get(session_id).map(|e| f(&e.row))
}

/// Every row's §7.2 JSON (S4's snapshot).
#[allow(dead_code)] // S4's snapshot route; integration tests via the lib
pub fn rows_json() -> Vec<serde_json::Value> {
    store().lock().rows.values().map(|e| e.row.to_json()).collect()
}

/// Every row's (session id, holds Keep awake, last evidence) right now
/// (Keep awake's re-read after a lag, A30).
pub fn keep_awake_view() -> Vec<(String, bool, Option<i64>)> {
    let now = now_ms();
    store()
        .lock()
        .rows
        .iter()
        .map(|(sid, e)| (sid.clone(), fold::holds_awake(&e.row, now, KEEP_AWAKE_STALE_MS), e.row.evidence_at))
        .collect()
}

/// Drop every row (integration tests share the process-wide store).
#[allow(dead_code)] // called via the LIB target by integration tests
pub fn clear_for_tests() {
    let mut st = store().lock();
    st.rows.clear();
    st.touched.clear();
    crate::activity_transcript::clear_for_tests();
}

/// Start the store, once per process: the hook-plane consumer and the
/// timer (row deadlines + the 10 s liveness sweep). Both are plain
/// threads, so they outlive any one tokio runtime (the in-process test
/// harness calls this too). Call before boot opens the gate.
pub fn spawn() {
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(|| {
        let mut rx = crate::hook_ingest::subscribe();
        let ingest = std::thread::Builder::new().name("activity-ingest".into()).spawn(move || {
            use tokio::sync::broadcast::error::RecvError;
            loop {
                match rx.blocking_recv() {
                    Ok(ev) => apply_ingest(&ev),
                    Err(RecvError::Lagged(n)) => {
                        // Lost envelopes: rows may be behind until the
                        // next evidence; decay still bounds a missed end.
                        log_debug!("[activity] hook ingest lagged {n} envelopes");
                    }
                    Err(RecvError::Closed) => return,
                }
            }
        });
        if let Err(e) = ingest {
            log_debug!("[activity] could not start the ingest thread: {e}");
        }
        let timer = std::thread::Builder::new().name("activity-timer".into()).spawn(|| {
            let mut sweep_at = std::time::Instant::now() + LIVENESS_SWEEP;
            loop {
                let now = now_ms();
                let until_deadline = next_deadline()
                    .map(|t| Duration::from_millis((t - now).max(0) as u64))
                    .unwrap_or(LIVENESS_SWEEP);
                let wait = until_deadline.min(sweep_at.saturating_duration_since(std::time::Instant::now()));
                // A new input may have moved the earliest deadline: wake,
                // tick whatever is due, and re-arm.
                wait_for_wake(wait);
                tick_all(now_ms());
                if std::time::Instant::now() >= sweep_at {
                    sweep_liveness();
                    sweep_at = std::time::Instant::now() + LIVENESS_SWEEP;
                }
            }
        });
        if let Err(e) = timer {
            log_debug!("[activity] could not start the timer thread: {e}");
        }
        crate::activity_transcript::spawn();
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use k2_core::agent_hooks::envelope::{self, HookHeaders, HookSource};

    static LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

    fn hook(sid: &str, body: &str) -> envelope::HookEnvelope {
        envelope::parse(
            &HookHeaders {
                pane: sid.to_string(),
                agent_pid: Some(100),
                source: HookSource::Claude,
                hook_version: Some(2),
                cli_version: None,
                truncated: false,
                event_hint: None,
            },
            body.as_bytes(),
        )
        .expect("parse")
    }

    /// The Companion report, agents page: the snapshot row of a pane whose
    /// `claude` runs its work as a background job. The job's hooks (a
    /// verified descendant of the owner, DA14-4a) drive the row: working,
    /// then `subagents_running` with live child counts after its Stop,
    /// with `evidenceAt` following them. Before, they were all foreign and
    /// the row sat on idle / `turn_done` with no children.
    #[test]
    fn descendant_hooks_drive_the_snapshot_row() {
        use k2_core::agent_hooks::owner::{self, OwnerCheckMode, PaneFacts, PaneOwner, ProcInfo, ProcessTable, Sender, Verdict};
        struct Procs;
        impl ProcessTable for Procs {
            fn info(&self, pid: i32) -> Option<ProcInfo> {
                // PTY child + owner 100; `claude daemon run` 110 →
                // `bg-pty-host` 120 → `bg-spare` 130.
                let ppid = match pid {
                    100 => 10,
                    110 => 100,
                    120 => 110,
                    130 => 120,
                    10 => 1,
                    _ => return None,
                };
                Some(ProcInfo { pid, ppid, start_time: pid as u64 })
            }
        }
        let _g = LOCK.lock();
        k2_core::test_isolation::assert_no_prod_env();
        let _db = k2_core::db::init_for_tests();
        let sid = "44444444-4444-4444-8444-444444444444";
        register(SessionFacts {
            session_id: sid.into(),
            agent_name: "tab-bgjob".into(),
            cwd: None,
            // Not a transcript harness: no follower is tracked, so nothing
            // looks for a transcript under any HOME.
            program: Some("bgjob-test".into()),
        });
        let facts = PaneFacts { child_pid: Some(100), known_conversation_id: None };
        let mut claim = PaneOwner::default();
        let mut feed = |pid: i32, body: &str, t: i64| {
            let mut env = envelope::parse(
                &HookHeaders {
                    pane: sid.to_string(),
                    agent_pid: Some(pid),
                    source: HookSource::Claude,
                    hook_version: Some(2),
                    cli_version: None,
                    truncated: false,
                    event_hint: None,
                },
                body.as_bytes(),
            )
            .expect("parse");
            let out = owner::check(&mut claim, Some(&facts), &Sender::of(&env), &Procs, OwnerCheckMode::Ancestry, t);
            match out.verdict {
                Verdict::Owner => {}
                Verdict::Descendant => env.from_descendant = true,
                other => panic!("pid {pid} {body}: {other:?}"),
            }
            apply_at(sid, Evidence::Hook(&env), t);
        };
        feed(100, r#"{"hook_event_name":"SessionStart","source":"resume","session_id":"lead"}"#, 1_000);
        feed(100, r#"{"hook_event_name":"UserPromptSubmit","prompt_id":"p1","session_id":"lead"}"#, 1_100);
        feed(100, r#"{"hook_event_name":"Stop","prompt_id":"p1","session_id":"lead","background_tasks":[],"session_crons":[]}"#, 1_200);
        let row = row_json(sid).expect("row");
        assert_eq!((row["display"].as_str(), row["reason"].as_str()), (Some("idle"), Some("turn_done")), "the report's state: {row}");

        feed(130, r#"{"hook_event_name":"SessionStart","source":"startup","session_id":"bg"}"#, 2_000);
        feed(130, r#"{"hook_event_name":"UserPromptSubmit","prompt_id":"b1","session_id":"bg"}"#, 2_100);
        feed(130, r#"{"hook_event_name":"PreToolUse","prompt_id":"b1","session_id":"bg","tool_name":"Task","tool_use_id":"tb-1","tool_input":{"description":"scan","prompt":"Synthetic.","run_in_background":true}}"#, 2_200);
        feed(130, r#"{"hook_event_name":"SubagentStart","session_id":"bg","agent_id":"a-b1","agent_type":"general-purpose"}"#, 2_300);
        let row = row_json(sid).expect("row");
        assert_eq!((row["display"].as_str(), row["reason"].as_str()), (Some("working"), Some("tool_running")), "{row}");
        assert_eq!(row["children"]["subagents"], 1, "{row}");
        feed(130, r#"{"hook_event_name":"PostToolUse","prompt_id":"b1","session_id":"bg","tool_name":"Task","tool_use_id":"tb-1","tool_input":{},"tool_response":{"isAsync":true,"agentId":"a-b1","status":"async_launched"}}"#, 2_400);
        feed(
            130,
            r#"{"hook_event_name":"Stop","prompt_id":"b1","session_id":"bg","background_tasks":[{"id":"a-b1","type":"subagent","status":"running"}],"session_crons":[]}"#,
            2_500,
        );
        let row = row_json(sid).expect("row");
        assert_eq!(row["display"], "working", "{row}");
        assert_eq!(row["reason"], "subagents_running", "{row}");
        assert_eq!(row["lead"]["state"], "idle", "{row}");
        assert_eq!(row["children"]["subagents"], 1, "{row}");
        assert_eq!(row["counts"]["subagents"], 1, "{row}");
        assert_eq!(row["counts"]["tools"], 1, "the background job's Task call counts: {row}");
        assert_eq!(row["evidenceAt"], 2_500, "evidence follows the background job: {row}");
        unregister(sid);
    }

    #[test]
    fn harness_names_come_from_the_spawn_command() {
        assert_eq!(harness_for(Some("/opt/bin/claude")), "claude");
        assert_eq!(harness_for(Some("codex")), "codex");
        assert_eq!(harness_for(Some("/usr/bin/cat")), "cat");
        assert_eq!(harness_for(None), "shell");
    }

    #[test]
    fn rows_live_between_register_and_unregister_and_publish_changes() {
        let _g = LOCK.lock();
        k2_core::test_isolation::assert_no_prod_env();
        let _db = k2_core::db::init_for_tests();
        let sid = "33333333-3333-4333-8333-333333333333";
        let mut rx = subscribe();
        register(SessionFacts {
            session_id: sid.into(),
            agent_name: "tab-x".into(),
            cwd: None,
            program: Some("claude".into()),
        });
        assert_eq!(row_json(sid).expect("row")["reason"], "unconfirmed");
        apply_at(sid, Evidence::Hook(&hook(sid, r#"{"hook_event_name":"UserPromptSubmit","prompt_id":"p"}"#)), 1_000);
        let ev = loop {
            let ev = rx.try_recv().expect("a row event");
            if ev.session_id == sid {
                break ev;
            }
        };
        assert_eq!(ev.row.as_ref().expect("row")["display"], "working");
        assert!(ev.holds_awake);
        unregister(sid);
        assert!(row_json(sid).is_none());
        let removed = loop {
            let ev = rx.try_recv().expect("a removal event");
            if ev.session_id == sid && ev.row.is_none() {
                break ev;
            }
        };
        assert!(!removed.holds_awake);
        // Input for a session with no row is ignored.
        apply_at(sid, Evidence::PtyExited, 2_000);
        assert!(row_json(sid).is_none());
    }

    /// k2-ptyd P37: a working row carried across a restart comes back
    /// working (same lead, same evidence, confirmed); a stale, garbled or
    /// future-dated carry leaves the fresh unconfirmed row (DA30).
    #[test]
    fn a_carried_row_seeds_the_adopted_row_only_while_fresh() {
        let _g = LOCK.lock();
        k2_core::test_isolation::assert_no_prod_env();
        let _db = k2_core::db::init_for_tests();
        let sid = "44444444-4444-4444-8444-444444444444";
        let facts = || SessionFacts { session_id: sid.into(), agent_name: "tab-p37".into(), cwd: None, program: Some("claude".into()) };
        register(facts());
        let t0 = now_ms() - 5_000;
        apply_at(sid, Evidence::Hook(&hook(sid, r#"{"hook_event_name":"UserPromptSubmit","prompt_id":"p37"}"#)), t0);
        let before = row_json(sid).expect("row");
        assert_eq!(before["display"], "working");
        assert_eq!(before["confirmed"], true);
        let row = with_row(sid, |r| r.clone()).expect("row");
        let saved_at = now_ms();
        let raw = serde_json::to_vec(&CarriedRow { v: 1, saved_at, row }).expect("serialize");

        // The next daemon registers the adopted session: fresh, unconfirmed.
        unregister(sid);
        register(facts());
        assert_eq!(row_json(sid).expect("row")["reason"], "unconfirmed");
        // Too old → not seeded.
        assert!(!restore_from(sid, &raw, saved_at + CARRIED_ROW_MAX_AGE_MS + 1));
        assert_eq!(row_json(sid).expect("row")["reason"], "unconfirmed");
        // From the future (a clock jump) → not seeded.
        assert!(!restore_from(sid, &raw, saved_at - 1));
        // Garbage → not seeded.
        assert!(!restore_from(sid, b"{not json", saved_at));
        assert_eq!(row_json(sid).expect("row")["reason"], "unconfirmed");
        // Fresh → the same row as before the restart.
        let mut rx = subscribe();
        assert!(restore_from(sid, &raw, saved_at + 1_000));
        let after = row_json(sid).expect("row");
        for k in ["display", "lead", "confirmed", "evidenceAt", "reason", "turnStartedAt", "rev"] {
            assert_eq!(after[k], before[k], "{k} after the restart");
        }
        let ev = loop {
            let ev = rx.try_recv().expect("an event for the seeded row");
            if ev.session_id == sid {
                break ev;
            }
        };
        assert!(ev.holds_awake, "a carried working row holds Keep awake again");
        unregister(sid);
    }

    /// F7: a carried row is read only through O_NOFOLLOW and up to the cap.
    #[test]
    fn carried_rows_are_read_capped_and_never_through_a_symlink() {
        let d = std::env::temp_dir().join(format!("k2row-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let ok = d.join("ok.json");
        std::fs::write(&ok, b"{}").unwrap();
        assert_eq!(read_capped(&ok, CARRIED_ROW_MAX_BYTES).as_deref(), Some(&b"{}"[..]));
        let big = d.join("big.json");
        std::fs::write(&big, vec![b' '; (CARRIED_ROW_MAX_BYTES + 1) as usize]).unwrap();
        assert!(read_capped(&big, CARRIED_ROW_MAX_BYTES).is_none(), "over the cap");
        let link = d.join("link.json");
        std::os::unix::fs::symlink(&ok, &link).unwrap();
        assert!(read_capped(&link, CARRIED_ROW_MAX_BYTES).is_none(), "never through a symlink");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn carried_row_paths_stay_in_their_dir() {
        assert!(carried_row_path("44444444-4444-4444-8444-444444444444").is_some());
        for bad in ["", "../x", "a/b", "x.json", "a b"] {
            assert!(carried_row_path(bad).is_none(), "{bad:?}");
        }
    }
}
