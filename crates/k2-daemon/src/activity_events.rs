//! `activity_changed` and the activity snapshot
//! (prd-daemon-activity-and-thread-working-v1 S4: RL1–RL6, A27–A32).
//!
//! The activity store (`activity_store`) holds one row per live v2
//! session. This module publishes it:
//! - **Rollup (RL1).** Rows fold into one entry per workspace: the
//!   highest display across its sessions, ranked
//!   `waiting > working > monitoring > unverifiable > idle`, the count per
//!   display, and `since` = the earliest `turnStartedAt` among its
//!   `working` sessions. A room is a workspace, so the same rollup serves
//!   Home, Zen, rooms and apps. A row belongs to the workspace the store
//!   resolved at registration (longest registered project prefix of the
//!   session cwd); a row with no project rolls up under its own path.
//! - **Live and busy (0.46.0, plan-agent-status-unify-v1 D1).** The rollup
//!   also counts `live` (live PTYs under the workspace: every v2 session
//!   row whose PTY has not exited, plus the legacy `TerminalManager`
//!   shells resolved by the same longest-prefix rule) and `busy` (rows
//!   mid-turn or with subagents still running, whatever their display, so
//!   "working" and "needs you" can show together). Legacy shells have no
//!   row and no event of their own: the publisher re-reads them on a
//!   create/kill nudge, every [`SHELL_CHECK_MS`] and before a snapshot,
//!   and sends a workspace-only frame (`row` and `removed` null) for each
//!   workspace whose shell count changed.
//! - **One event (RL2).** Every row change or removal goes out as one
//!   `activity_changed` frame on the session-events bus, carrying the row
//!   and its workspace rollup, a store-wide `seq` and the daemon's
//!   `instanceId`. Each row sends at most one frame per [`COALESCE_MS`];
//!   the last state always goes out (a trailing frame). `seq` is assigned
//!   under the same lock that sends, so the bus sees it gap-free.
//! - **Snapshot (RL3).** [`snapshot`] is every row, every rollup, `seq`,
//!   `instanceId` and `staleAfterSecs`, read under that lock, so a client
//!   that pulls it and then sees `seq + 1` has missed nothing (RL4).
//! - **Compat (RL5, A29).** For one release the store's state also feeds
//!   the old `session_activity_changed` (`working | idle | permission`),
//!   no longer the title observer; `agent_status_changed` /
//!   `agent:lifecycle` already come from the store (S2). The token-ledger
//!   idle scan runs on every turn end.
//!
//! Inputs: [`activity_store::subscribe`]. A lag on that bus re-reads the
//! store (A30): every live row is re-sent and rows that vanished are sent
//! as removals.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::OnceLock;
use std::time::Duration;

use parking_lot::Mutex;
use serde_json::{json, Value};

use k2_core::activity::{Display, TurnEnded};
use k2_core::log_debug;

use crate::activity_store::{self, RowEvent};
use crate::session_events::{self, SessionEvent};

/// RL2: one frame per row per this window (the last state always goes out).
pub const COALESCE_MS: i64 = 100;

/// D1: how often the publisher re-reads the legacy shells when nothing
/// nudged it (a shell's process can exit without a route call).
pub const SHELL_CHECK_MS: i64 = 5_000;

/// One row as the rollup reads it (§7.2 JSON).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowView {
    pub session_id: String,
    pub agent_name: String,
    pub project_id: Option<String>,
    pub workspace_path: Option<String>,
    pub display: Display,
    pub turn_started_at: Option<i64>,
    /// The row's `counts` (§7.2, 0.45.2): live subagents and this turn's
    /// lead tool calls and shell commands.
    pub counts: ActivityCounts,
    /// D1: the row's `lead.state` is `working` (mid-turn).
    pub lead_working: bool,
    /// D1: the session's PTY is still running (the row's `reason` is not
    /// `pty_exited`). A reaped session has no row at all.
    pub live: bool,
}

/// Numbers only: live subagents, this turn's lead tool calls, and the
/// shell commands among them (the row's `counts`; the app frame's too).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ActivityCounts {
    pub subagents: u64,
    pub tools: u64,
    pub commands: u64,
}

impl ActivityCounts {
    /// The row's `counts` object. A row without one (none today) reads
    /// as zeros.
    pub fn from_json(v: Option<&Value>) -> Self {
        let n = |k: &str| v.and_then(|c| c.get(k)).and_then(Value::as_u64).unwrap_or(0);
        Self { subagents: n("subagents"), tools: n("tools"), commands: n("commands") }
    }

    pub fn to_json(self) -> Value {
        json!({ "subagents": self.subagents, "tools": self.tools, "commands": self.commands })
    }
}

/// The wire word for a display (`Display::as_str`) back to the enum.
pub fn parse_display(s: &str) -> Option<Display> {
    Some(match s {
        "working" => Display::Working,
        "monitoring" => Display::Monitoring,
        "waiting" => Display::Waiting,
        "idle" => Display::Idle,
        "unverifiable" => Display::Unverifiable,
        _ => return None,
    })
}

impl RowView {
    pub fn from_json(v: &Value) -> Option<Self> {
        let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
        Some(Self {
            session_id: s("sessionId")?,
            agent_name: s("agentName").unwrap_or_default(),
            project_id: s("projectId"),
            workspace_path: s("workspacePath"),
            display: parse_display(v.get("display")?.as_str()?)?,
            turn_started_at: v.get("turnStartedAt").and_then(Value::as_i64),
            counts: ActivityCounts::from_json(v.get("counts")),
            lead_working: v.pointer("/lead/state").and_then(Value::as_str) == Some("working"),
            live: v.get("reason").and_then(Value::as_str) != Some("pty_exited"),
        })
    }

    /// D1 `busy`: the session is mid-turn or still has subagents running,
    /// even when its lead (or a subagent) waits on you. Counted only on a
    /// `working` or `waiting` row: a stale (`unverifiable`) or unconfirmed
    /// row never spins.
    pub fn is_busy(&self) -> bool {
        match self.display {
            Display::Working => true,
            Display::Waiting => self.lead_working || self.counts.subagents > 0,
            Display::Monitoring | Display::Unverifiable | Display::Idle => false,
        }
    }
}

/// D1: one live legacy `TerminalManager` PTY (a plain shell pane), keyed
/// like a row: the registered project that owns its cwd, else its cwd.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellView {
    pub project_id: Option<String>,
    pub workspace_path: Option<String>,
}

impl ShellView {
    fn key(&self) -> WorkspaceKey {
        WorkspaceKey { project_id: self.project_id.clone(), workspace_path: self.workspace_path.clone() }
    }
}

/// Every live legacy shell's cwd (`/cli/terminal/list-running`'s list).
pub fn legacy_shell_cwds() -> Vec<String> {
    let manager = k2_core::terminal::shared();
    let manager = manager.lock();
    manager.list_terminal_ids().into_iter().map(|(_, cwd)| cwd).collect()
}

/// Resolve legacy shell cwds onto workspaces (one project read).
fn resolve_shells(cwds: &[String]) -> Vec<ShellView> {
    if cwds.is_empty() {
        return Vec::new();
    }
    shells_in(cwds, &activity_store::registered_projects())
}

/// [`resolve_shells`] against a given project list (`(id, path)`).
pub fn shells_in(cwds: &[String], projects: &[(String, String)]) -> Vec<ShellView> {
    cwds.iter()
        .filter(|c| !c.is_empty())
        .map(|cwd| match activity_store::project_in(cwd, projects) {
            Some((pid, path)) => ShellView { project_id: Some(pid), workspace_path: Some(path) },
            None => ShellView { project_id: None, workspace_path: Some(cwd.clone()) },
        })
        .collect()
}

/// RL1 ranking: `waiting > working > monitoring > unverifiable > idle`.
pub fn rank(d: Display) -> u8 {
    match d {
        Display::Waiting => 4,
        Display::Working => 3,
        Display::Monitoring => 2,
        Display::Unverifiable => 1,
        Display::Idle => 0,
    }
}

/// The pre-0.45 three-word vocabulary for a display: compat
/// `session_activity_changed` (RL5), `presence/summary` `agentActivity`
/// and `ops/overview` `agentStatus` (A31). Older clients drop any other
/// word (`host-pool.ts` `parsePresenceActivity`).
pub fn legacy_word(d: Display) -> &'static str {
    match d {
        Display::Working | Display::Monitoring => "working",
        Display::Waiting => "permission",
        Display::Idle | Display::Unverifiable => "idle",
    }
}

/// Which workspace a row rolls up under: its project when the store
/// resolved one, else its own path.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct WorkspaceKey {
    pub project_id: Option<String>,
    pub workspace_path: Option<String>,
}

impl WorkspaceKey {
    pub fn of(row: &RowView) -> Option<Self> {
        if row.project_id.is_none() && row.workspace_path.is_none() {
            return None;
        }
        Some(Self { project_id: row.project_id.clone(), workspace_path: row.workspace_path.clone() })
    }

    pub fn matches(&self, row: &RowView) -> bool {
        self.matches_parts(row.project_id.as_deref(), row.workspace_path.as_deref())
    }

    pub fn matches_shell(&self, shell: &ShellView) -> bool {
        self.matches_parts(shell.project_id.as_deref(), shell.workspace_path.as_deref())
    }

    fn matches_parts(&self, project_id: Option<&str>, workspace_path: Option<&str>) -> bool {
        match &self.project_id {
            Some(pid) => project_id == Some(pid.as_str()),
            None => project_id.is_none() && workspace_path == self.workspace_path.as_deref(),
        }
    }
}

/// Sessions per display in one workspace.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DisplayCounts {
    pub working: usize,
    pub monitoring: usize,
    pub waiting: usize,
    pub unverifiable: usize,
    pub idle: usize,
}

/// RL1: one workspace's rollup (§7.3 `workspace`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rollup {
    pub project_id: Option<String>,
    pub workspace_path: Option<String>,
    pub display: Display,
    pub counts: DisplayCounts,
    /// The earliest `turnStartedAt` among the `working` sessions.
    pub since: Option<i64>,
    /// D1: live PTYs under the workspace (session rows whose PTY runs,
    /// plus legacy shells). 0 = everything reaped.
    pub live: usize,
    /// D1: sessions mid-turn or with subagents running ([`RowView::is_busy`]).
    pub busy: usize,
}

impl Rollup {
    pub fn to_json(&self) -> Value {
        let c = self.counts;
        json!({
            "projectId": self.project_id,
            "workspacePath": self.workspace_path,
            "display": self.display.as_str(),
            "counts": {
                "working": c.working,
                "monitoring": c.monitoring,
                "waiting": c.waiting,
                "unverifiable": c.unverifiable,
                "idle": c.idle,
            },
            "since": self.since,
            "live": self.live,
            "busy": self.busy,
        })
    }
}

/// RL1: fold the rows (and, D1, the legacy shells) `key` owns. A
/// workspace with no rows is idle.
pub fn rollup(key: &WorkspaceKey, rows: &[RowView], shells: &[ShellView]) -> Rollup {
    let mut out = Rollup {
        project_id: key.project_id.clone(),
        workspace_path: key.workspace_path.clone(),
        display: Display::Idle,
        counts: DisplayCounts::default(),
        since: None,
        live: shells.iter().filter(|s| key.matches_shell(s)).count(),
        busy: 0,
    };
    for row in rows.iter().filter(|r| key.matches(r)) {
        if out.workspace_path.is_none() {
            out.workspace_path = row.workspace_path.clone();
        }
        if row.live {
            out.live += 1;
        }
        if row.is_busy() {
            out.busy += 1;
        }
        match row.display {
            Display::Working => out.counts.working += 1,
            Display::Monitoring => out.counts.monitoring += 1,
            Display::Waiting => out.counts.waiting += 1,
            Display::Unverifiable => out.counts.unverifiable += 1,
            Display::Idle => out.counts.idle += 1,
        }
        if rank(row.display) > rank(out.display) {
            out.display = row.display;
        }
        if row.display == Display::Working {
            if let Some(t) = row.turn_started_at {
                out.since = Some(out.since.map_or(t, |s| s.min(t)));
            }
        }
    }
    out
}

/// Every workspace that has at least one row or legacy shell, ordered by
/// key.
pub fn rollups(rows: &[RowView], shells: &[ShellView]) -> Vec<Rollup> {
    let mut keys: Vec<WorkspaceKey> = Vec::new();
    for row in rows {
        let Some(k) = WorkspaceKey::of(row) else { continue };
        // Rows of one project may carry different paths; the project id
        // is the key.
        if !keys.iter().any(|have| have.matches(row)) {
            keys.push(k);
        }
    }
    for shell in shells {
        if !keys.iter().any(|have| have.matches_shell(shell)) {
            keys.push(shell.key());
        }
    }
    keys.sort();
    keys.iter().map(|k| rollup(k, rows, shells)).collect()
}

/// D1: live legacy shells per workspace key.
fn shell_counts(shells: &[ShellView]) -> BTreeMap<WorkspaceKey, usize> {
    let mut out: BTreeMap<WorkspaceKey, usize> = BTreeMap::new();
    for s in shells {
        *out.entry(s.key()).or_insert(0) += 1;
    }
    out
}

/// D1: the workspaces whose legacy shell count differs between `before`
/// and `after`.
fn shell_changes(before: &[ShellView], after: &[ShellView]) -> Vec<WorkspaceKey> {
    let (a, b) = (shell_counts(before), shell_counts(after));
    let mut keys: Vec<WorkspaceKey> = a.keys().chain(b.keys()).cloned().collect();
    keys.sort();
    keys.dedup();
    keys.retain(|k| a.get(k) != b.get(k));
    keys
}

/// `turnEnded` on the wire (§7.3, plus the end `reason` for S5's toast
/// rules).
fn turn_json(t: TurnEnded) -> Value {
    json!({ "outcome": t.outcome.as_str(), "reason": t.reason.as_str(), "at": t.at })
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn all_views() -> Vec<RowView> {
    activity_store::rows_json().iter().filter_map(RowView::from_json).collect()
}

/// The registered project at exactly `path` (a removed row the publisher
/// never saw change).
fn key_for_path(path: Option<&str>) -> Option<WorkspaceKey> {
    let path = path.filter(|p| !p.is_empty())?;
    let project_id = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.query_row(
            "SELECT id FROM projects WHERE path = ?1",
            rusqlite::params![path],
            |r| r.get::<_, String>(0),
        )
        .ok()
    };
    Some(WorkspaceKey { project_id, workspace_path: Some(path.to_string()) })
}

/// A29: the token ledger rescans the workspace of a session whose lead
/// just went idle: the session's own cwd while it is live (the CLI keys
/// its transcript dir by cwd), else the row's workspace path.
pub fn idle_scan_cwd(ev: &RowEvent) -> Option<String> {
    ev.turn_ended?;
    let live_cwd = k2_core::session::SessionId::parse(&ev.session_id)
        .and_then(|id| crate::v2_session_map::lookup_by_session_id(&id))
        .and_then(|s| s.cwd.as_ref().map(|p| p.to_string_lossy().into_owned()));
    live_cwd.or_else(|| ev.workspace_path.clone()).filter(|p| !p.is_empty())
}

struct Pending {
    due: i64,
    turn_ended: Option<TurnEnded>,
}

/// What the publisher last told clients about one session.
struct Known {
    agent_name: String,
    key: Option<WorkspaceKey>,
}

#[derive(Default)]
struct Publisher {
    seq: u64,
    last_emit: HashMap<String, i64>,
    pending: HashMap<String, Pending>,
    /// RL5: the last compat `session_activity_changed` word per session.
    compat: HashMap<String, &'static str>,
    known: HashMap<String, Known>,
    /// D1: the legacy shells as of the last check, and when that was.
    shells: Vec<ShellView>,
    shells_checked_at: i64,
}

impl Publisher {
    fn send(&mut self, row: Option<Value>, removed: Option<String>, turn: Option<TurnEnded>, workspace: Option<Value>) {
        self.seq += 1;
        let _ = session_events::emit(SessionEvent::ActivityChanged {
            seq: self.seq,
            instance_id: crate::boot_status::instance_id().to_string(),
            row,
            removed,
            turn_ended: turn.map(turn_json),
            workspace,
        });
    }

    fn send_compat(&mut self, session_id: &str, agent_name: &str, workspace_path: Option<&str>, word: &'static str) {
        self.compat.insert(session_id.to_string(), word);
        let _ = session_events::emit(SessionEvent::SessionActivityChanged {
            workspace_path: workspace_path.unwrap_or_default().to_string(),
            agent_name: agent_name.to_string(),
            pane_group_id: session_events::pane_group_id_from_agent(agent_name),
            status: word.to_string(),
        });
    }

    /// Send the row's CURRENT state now (RL2: the last state always goes
    /// out), merged with any coalesced turn end.
    fn emit_row(&mut self, sid: &str, turn: Option<TurnEnded>, now: i64) {
        self.pending.remove(sid);
        let Some(row) = activity_store::row_json(sid) else {
            // Removed since; its removal frame follows.
            return;
        };
        let Some(view) = RowView::from_json(&row) else {
            log_debug!("[activity-events] unreadable row for {sid}");
            return;
        };
        let key = WorkspaceKey::of(&view);
        let workspace = key.as_ref().map(|k| rollup(k, &all_views(), &self.shells).to_json());
        self.known.insert(sid.to_string(), Known { agent_name: view.agent_name.clone(), key });
        self.send(Some(row), None, turn, workspace);
        self.last_emit.insert(sid.to_string(), now);
        let word = legacy_word(view.display);
        let last = self.compat.get(sid).copied();
        // A row's first word is never a bare `idle` (nothing to clear).
        if last != Some(word) && !(last.is_none() && word == "idle") {
            self.send_compat(sid, &view.agent_name, view.workspace_path.as_deref(), word);
        }
    }

    fn emit_removed(&mut self, sid: &str, agent_name: &str, workspace_path: Option<&str>) {
        self.pending.remove(sid);
        self.last_emit.remove(sid);
        let known = self.known.remove(sid);
        if matches!(self.compat.remove(sid), Some("working" | "permission")) {
            let name = known.as_ref().map_or(agent_name, |k| k.agent_name.as_str()).to_string();
            self.send_compat(sid, &name, workspace_path, "idle");
            self.compat.remove(sid);
        }
        let key = known.and_then(|k| k.key).or_else(|| key_for_path(workspace_path));
        let workspace = key.map(|k| rollup(&k, &all_views(), &self.shells).to_json());
        self.send(None, Some(sid.to_string()), None, workspace);
    }

    /// One store event. True when a trailing frame was scheduled (wake the
    /// flusher).
    fn on_event(&mut self, ev: &RowEvent, now: i64) -> bool {
        if ev.row.is_none() {
            self.emit_removed(&ev.session_id, &ev.agent_name, ev.workspace_path.as_deref());
            return false;
        }
        if let Some(cwd) = idle_scan_cwd(ev) {
            crate::token_usage_scan::note_idle_workspace(&cwd);
        }
        self.on_change(&ev.session_id, ev.turn_ended, now)
    }

    fn on_change(&mut self, sid: &str, turn: Option<TurnEnded>, now: i64) -> bool {
        let merged = turn.or_else(|| self.pending.get(sid).and_then(|p| p.turn_ended));
        match self.last_emit.get(sid).map(|t| t + COALESCE_MS) {
            Some(due) if due > now => {
                self.pending.insert(sid.to_string(), Pending { due, turn_ended: merged });
                true
            }
            _ => {
                self.emit_row(sid, merged, now);
                false
            }
        }
    }

    /// Send every trailing frame that is due; returns the next due time.
    fn flush_due(&mut self, now: i64) -> Option<i64> {
        let due: Vec<(String, Option<TurnEnded>)> = self
            .pending
            .iter()
            .filter(|(_, p)| p.due <= now)
            .map(|(sid, p)| (sid.clone(), p.turn_ended))
            .collect();
        for (sid, turn) in due {
            self.emit_row(&sid, turn, now);
        }
        self.pending.values().map(|p| p.due).min()
    }

    /// D1: re-read the legacy shells. Each workspace whose shell count
    /// changed gets a workspace-only frame (`row` and `removed` null) with
    /// its new rollup. Reads nothing more when there were and are none.
    fn reconcile_shells(&mut self, now: i64) {
        self.shells_checked_at = now;
        let cwds = legacy_shell_cwds();
        if cwds.is_empty() && self.shells.is_empty() {
            return;
        }
        let next = resolve_shells(&cwds);
        self.apply_shells(next);
    }

    /// D1: take `next` as the legacy shell set and announce the changes.
    fn apply_shells(&mut self, next: Vec<ShellView>) {
        let changed = shell_changes(&self.shells, &next);
        self.shells = next;
        if changed.is_empty() {
            return;
        }
        let views = all_views();
        for key in changed {
            let workspace = rollup(&key, &views, &self.shells).to_json();
            self.send(None, None, None, Some(workspace));
        }
    }

    /// A30: the store bus lagged. Re-read the store: rows that vanished
    /// go out as removals, every live row is re-sent.
    fn resync(&mut self, now: i64) -> bool {
        let live: Vec<RowView> = all_views();
        let live_ids: HashSet<&str> = live.iter().map(|r| r.session_id.as_str()).collect();
        let gone: Vec<String> = self.known.keys().filter(|s| !live_ids.contains(s.as_str())).cloned().collect();
        for sid in gone {
            let (name, path) = self
                .known
                .get(&sid)
                .map(|k| (k.agent_name.clone(), k.key.as_ref().and_then(|k| k.workspace_path.clone())))
                .unwrap_or_default();
            self.emit_removed(&sid, &name, path.as_deref());
        }
        let mut wake = false;
        for row in &live {
            wake |= self.on_change(&row.session_id, None, now);
        }
        wake
    }
}

fn publisher() -> &'static Mutex<Publisher> {
    static P: OnceLock<Mutex<Publisher>> = OnceLock::new();
    P.get_or_init(|| Mutex::new(Publisher::default()))
}

fn wake_cell() -> &'static (std::sync::Mutex<bool>, std::sync::Condvar) {
    static W: OnceLock<(std::sync::Mutex<bool>, std::sync::Condvar)> = OnceLock::new();
    W.get_or_init(|| (std::sync::Mutex::new(false), std::sync::Condvar::new()))
}

fn wake_flusher() {
    let (flag, cv) = wake_cell();
    if let Ok(mut woken) = flag.lock() {
        *woken = true;
        cv.notify_one();
    }
}

fn wait_for_wake(timeout: Duration) {
    let (flag, cv) = wake_cell();
    let Ok(guard) = flag.lock() else { return };
    let Ok((mut guard, _)) = cv.wait_timeout_while(guard, timeout, |woken| !*woken) else {
        return;
    };
    *guard = false;
}

/// RL2: a newly registered row (idle, unconfirmed, DA30) goes out too,
/// so every client's rollup counts include it before its first evidence.
/// Called right after `activity_store::register`.
pub fn note_registered(session_id: &str) {
    if publisher().lock().on_change(session_id, None, now_ms()) {
        wake_flusher();
    }
}

/// RL3 / §7.4: every row and every workspace rollup, or with `only` just
/// that workspace's rows and its rollup (idle when it has none). Read
/// under the publisher lock, so `seq` is the last frame these rows are at
/// least as new as.
pub fn snapshot(only: Option<&WorkspaceKey>) -> Value {
    let mut p = publisher().lock();
    // D1: the legacy shells as of now (a change goes out as a frame
    // first, so `seq` still covers it).
    p.reconcile_shells(now_ms());
    let mut rows: Vec<Value> = activity_store::rows_json();
    let mut views: Vec<RowView> = rows.iter().filter_map(RowView::from_json).collect();
    if let Some(key) = only {
        rows.retain(|r| RowView::from_json(r).is_some_and(|v| key.matches(&v)));
        views.retain(|v| key.matches(v));
    }
    rows.sort_by(|a, b| a["sessionId"].as_str().cmp(&b["sessionId"].as_str()));
    let workspaces: Vec<Value> = match only {
        Some(key) => vec![rollup(key, &views, &p.shells).to_json()],
        None => rollups(&views, &p.shells).iter().map(Rollup::to_json).collect(),
    };
    json!({
        "instanceId": crate::boot_status::instance_id(),
        "seq": p.seq,
        "serverNow": now_ms(),
        "staleAfterSecs": k2_core::activity::row::STALE_AFTER_MS / 1000,
        "rows": rows,
        "workspaces": workspaces,
    })
}

/// D1: a legacy shell was created or killed (`/cli/terminal/create`,
/// `/cli/terminal/kill`). Re-read the shells now so the workspace's `live`
/// goes out without waiting for the next check. The caller must not hold
/// the `TerminalManager` lock.
pub fn note_shells_changed() {
    publisher().lock().reconcile_shells(now_ms());
}

/// The current display of every live row, by session id (the server
/// readers: presence summary, ops overview, `/v1` busy check, A31).
pub fn displays() -> BTreeMap<String, (Display, Value)> {
    activity_store::rows_json()
        .into_iter()
        .filter_map(|row| {
            let v = RowView::from_json(&row)?;
            Some((v.session_id, (v.display, row)))
        })
        .collect()
}

/// Start the publisher, once per process: the store-event consumer and
/// the trailing-frame flusher. Plain threads, like the store's own, so
/// they outlive any one tokio runtime (the in-process test harness calls
/// this too).
pub fn spawn() {
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(|| {
        let mut rx = activity_store::subscribe();
        let consumer = std::thread::Builder::new().name("activity-publish".into()).spawn(move || {
            use tokio::sync::broadcast::error::RecvError;
            loop {
                let wake = match rx.blocking_recv() {
                    Ok(ev) => publisher().lock().on_event(&ev, now_ms()),
                    Err(RecvError::Lagged(n)) => {
                        log_debug!("[activity-events] store bus lagged {n}; re-reading the store");
                        publisher().lock().resync(now_ms())
                    }
                    Err(RecvError::Closed) => return,
                };
                if wake {
                    wake_flusher();
                }
            }
        });
        if let Err(e) = consumer {
            log_debug!("[activity-events] could not start the publisher: {e}");
        }
        let flusher = std::thread::Builder::new().name("activity-flush".into()).spawn(|| loop {
            let now = now_ms();
            let next = {
                let mut p = publisher().lock();
                if now - p.shells_checked_at >= SHELL_CHECK_MS {
                    p.reconcile_shells(now);
                }
                p.flush_due(now)
            };
            let wait = next.map_or(Duration::from_secs(1), |t| Duration::from_millis((t - now).max(1) as u64));
            wait_for_wake(wait);
        });
        if let Err(e) = flusher {
            log_debug!("[activity-events] could not start the flusher: {e}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(sid: &str, pid: Option<&str>, path: Option<&str>, d: Display, started: Option<i64>) -> RowView {
        RowView {
            session_id: sid.into(),
            agent_name: format!("tab-{sid}"),
            project_id: pid.map(str::to_string),
            workspace_path: path.map(str::to_string),
            display: d,
            turn_started_at: started,
            counts: ActivityCounts::default(),
            lead_working: d == Display::Working,
            live: true,
        }
    }

    /// T-S4c: ranking `waiting > working > monitoring > unverifiable >
    /// idle`, counts per display, `since` = earliest working turn start.
    #[test]
    fn rollup_ranks_counts_and_picks_the_earliest_working_turn() {
        use Display::*;
        let rows = vec![
            view("a", Some("p1"), Some("/w/one"), Working, Some(500)),
            view("b", Some("p1"), Some("/w/one"), Working, Some(300)),
            view("c", Some("p1"), Some("/w/one"), Idle, Some(100)),
            view("d", Some("p1"), Some("/w/one"), Monitoring, Some(50)),
            view("e", Some("p2"), Some("/w/two"), Unverifiable, Some(10)),
            view("f", Some("p2"), Some("/w/two"), Idle, None),
            view("g", None, Some("/loose"), Waiting, None),
            view("h", None, None, Working, Some(1)),
        ];
        let all = rollups(&rows, &[]);
        assert_eq!(all.len(), 3, "{all:?}");
        let p1 = all.iter().find(|r| r.project_id.as_deref() == Some("p1")).expect("p1");
        assert_eq!(p1.display, Working);
        assert_eq!(
            p1.counts,
            DisplayCounts { working: 2, monitoring: 1, waiting: 0, unverifiable: 0, idle: 1 }
        );
        assert_eq!(p1.since, Some(300), "earliest WORKING turn, not the idle or monitoring one");
        let p2 = all.iter().find(|r| r.project_id.as_deref() == Some("p2")).expect("p2");
        assert_eq!((p2.display, p2.since), (Unverifiable, None));
        let loose = all.iter().find(|r| r.project_id.is_none()).expect("path-keyed");
        assert_eq!((loose.display, loose.workspace_path.as_deref()), (Waiting, Some("/loose")));

        // Pairwise: the higher-ranked display always wins.
        let order = [Idle, Unverifiable, Monitoring, Working, Waiting];
        for (i, lo) in order.iter().enumerate() {
            for hi in &order[i..] {
                let rows = vec![view("x", Some("p"), None, *lo, Some(1)), view("y", Some("p"), None, *hi, Some(2))];
                let key = WorkspaceKey::of(&rows[0]).expect("key");
                assert_eq!(rollup(&key, &rows, &[]).display, *hi, "{lo:?} vs {hi:?}");
            }
        }
        // A workspace with no rows is idle with zero counts.
        let empty = rollup(&WorkspaceKey { project_id: Some("none".into()), workspace_path: None }, &rows, &[]);
        assert_eq!((empty.display, empty.counts, empty.since), (Idle, DisplayCounts::default(), None));
        let j = p1.to_json();
        assert_eq!(j["display"], "working");
        assert_eq!(j["counts"]["working"], 2);
        assert_eq!(j["since"], 300);
        assert_eq!(j["projectId"], "p1");
        assert_eq!(j["live"], 4, "every p1 row is a live PTY: {j}");
        assert_eq!(j["busy"], 2, "the two working rows: {j}");
    }

    /// D1 `live`: session rows whose PTY runs plus legacy shells, by the
    /// workspace's key; an exited PTY and another workspace's shell don't
    /// count.
    #[test]
    fn rollup_live_counts_agent_rows_and_plain_shells_across_sessions() {
        use Display::*;
        let mut exited = view("x", Some("p1"), Some("/w/one"), Idle, None);
        exited.live = false;
        let rows = vec![
            view("a", Some("p1"), Some("/w/one"), Working, Some(1)),
            view("b", Some("p1"), Some("/w/one"), Idle, None),
            exited,
            view("c", Some("p2"), Some("/w/two"), Idle, None),
        ];
        let shell = |pid: Option<&str>, path: &str| ShellView {
            project_id: pid.map(str::to_string),
            workspace_path: Some(path.to_string()),
        };
        let shells = vec![shell(Some("p1"), "/w/one"), shell(Some("p3"), "/w/three"), shell(None, "/tmp/loose")];
        let all = rollups(&rows, &shells);
        let find = |pred: &dyn Fn(&Rollup) -> bool| all.iter().find(|r| pred(r)).cloned().expect("rollup");
        let p1 = find(&|r| r.project_id.as_deref() == Some("p1"));
        assert_eq!(p1.live, 3, "2 running rows + 1 shell, not the exited row: {p1:?}");
        assert_eq!(p1.counts.idle, 2, "the exited row still counts as an idle row");
        let p2 = find(&|r| r.project_id.as_deref() == Some("p2"));
        assert_eq!((p2.live, p2.busy, p2.display), (1, 0, Idle));
        // A workspace with only a shell still gets a rollup (idle, live 1).
        let p3 = find(&|r| r.project_id.as_deref() == Some("p3"));
        assert_eq!((p3.live, p3.display, p3.counts), (1, Idle, DisplayCounts::default()));
        assert_eq!(p3.workspace_path.as_deref(), Some("/w/three"));
        let loose = find(&|r| r.project_id.is_none());
        assert_eq!((loose.live, loose.workspace_path.as_deref()), (1, Some("/tmp/loose")));
        assert_eq!(all.len(), 4, "{all:?}");
        // Reaped: no rows and no shells → live 0.
        let key = WorkspaceKey { project_id: Some("p1".into()), workspace_path: Some("/w/one".into()) };
        assert_eq!(rollup(&key, &[], &[]).live, 0);
        assert_eq!(rollup(&key, &[], &[]).to_json()["live"], 0);
    }

    /// D1 `busy`: a row mid-turn or with subagents running, whatever its
    /// display; a waiting lead with a running subagent is busy AND waiting.
    #[test]
    fn rollup_busy_counts_mid_turn_and_subagent_sessions() {
        use Display::*;
        let with = |sid: &str, d: Display, lead_working: bool, subagents: u64| {
            let mut v = view(sid, Some("p"), Some("/w"), d, Some(1));
            v.lead_working = lead_working;
            v.counts.subagents = subagents;
            v
        };
        // Lead waits on you while one subagent runs.
        let asks = with("a", Waiting, false, 1);
        assert!(asks.is_busy());
        let r = rollup(&WorkspaceKey::of(&asks).expect("key"), &[asks.clone()], &[]);
        assert_eq!((r.display, r.busy, r.counts.working), (Waiting, 1, 0));
        // A child waits while the lead is mid-turn: busy too.
        assert!(with("b", Waiting, true, 0).is_busy());
        // Waiting with nothing else running: not busy.
        assert!(!with("c", Waiting, false, 0).is_busy());
        // Working (a subagent or an owed notification) is always busy.
        assert!(with("d", Working, false, 0).is_busy());
        // Background shells only, stale or idle rows never spin.
        assert!(!with("e", Monitoring, false, 0).is_busy());
        assert!(!with("f", Unverifiable, true, 2).is_busy());
        assert!(!with("g", Idle, false, 0).is_busy());
        let rows = vec![
            asks,
            with("h", Working, true, 0),
            with("i", Monitoring, false, 0),
            with("j", Waiting, false, 0),
            with("k", Unverifiable, true, 0),
        ];
        let r = rollup(&WorkspaceKey::of(&rows[0]).expect("key"), &rows, &[]);
        assert_eq!((r.display, r.busy, r.live), (Waiting, 2, 5), "{r:?}");
        assert_eq!(r.to_json()["busy"], 2);
    }

    /// D1: the row JSON feeds `lead_working` and `live`.
    #[test]
    fn row_view_reads_lead_state_and_pty_exit() {
        let row = |lead: &str, reason: &str| {
            json!({
                "sessionId": "s", "agentName": "tab-s", "projectId": "p", "workspacePath": "/w",
                "display": "waiting", "lead": { "state": lead }, "reason": reason,
                "counts": { "subagents": 1, "tools": 0, "commands": 0 },
            })
        };
        let v = RowView::from_json(&row("working", "waiting_permission")).expect("row");
        assert!(v.lead_working && v.live && v.is_busy());
        let v = RowView::from_json(&row("waiting", "pty_exited")).expect("row");
        assert!(!v.lead_working && !v.live);
        assert_eq!(v.counts.subagents, 1);
    }

    /// D1: a legacy shell's cwd resolves to the registered workspace that
    /// owns it (longest prefix, so a nested workspace takes its own); one
    /// outside every workspace keys by its cwd. The shell diff names only
    /// workspaces whose count moved.
    #[test]
    fn legacy_shells_resolve_by_longest_prefix_and_diff_by_workspace() {
        let projects = vec![("parent".to_string(), "/w".to_string()), ("child".to_string(), "/w/app".to_string())];
        let cwds = vec!["/w/app/src".to_string(), "/w/docs".to_string(), "/else".to_string(), String::new()];
        let shells = shells_in(&cwds, &projects);
        assert_eq!(
            shells,
            vec![
                ShellView { project_id: Some("child".into()), workspace_path: Some("/w/app".into()) },
                ShellView { project_id: Some("parent".into()), workspace_path: Some("/w".into()) },
                ShellView { project_id: None, workspace_path: Some("/else".into()) },
            ]
        );
        let child = WorkspaceKey { project_id: Some("child".into()), workspace_path: Some("/w/app".into()) };
        let parent = WorkspaceKey { project_id: Some("parent".into()), workspace_path: Some("/w".into()) };
        assert_eq!(rollup(&child, &[], &shells).live, 1, "the nested shell counts for the child");
        assert_eq!(rollup(&parent, &[], &shells).live, 1, "not for the parent too");
        // One more shell in the child, the loose one gone.
        let after = shells_in(&["/w/app".to_string(), "/w/app/x".to_string(), "/w".to_string()], &projects);
        let changed = shell_changes(&shells, &after);
        let loose = WorkspaceKey { project_id: None, workspace_path: Some("/else".into()) };
        let mut want = vec![loose, child];
        want.sort();
        assert_eq!(changed, want);
        assert!(shell_changes(&after, &after).is_empty());
    }

    /// A31 / RL5: the old three words.
    #[test]
    fn legacy_words_keep_older_clients_vocabulary() {
        assert_eq!(legacy_word(Display::Working), "working");
        assert_eq!(legacy_word(Display::Monitoring), "working");
        assert_eq!(legacy_word(Display::Waiting), "permission");
        assert_eq!(legacy_word(Display::Unverifiable), "idle");
        assert_eq!(legacy_word(Display::Idle), "idle");
        for d in ["working", "monitoring", "waiting", "idle", "unverifiable"] {
            assert_eq!(parse_display(d).expect(d).as_str(), d);
        }
        assert!(parse_display("busy").is_none());
    }

    /// T-S4k (A29): a turn end asks the token ledger for a scan of that
    /// workspace; a plain change doesn't.
    #[test]
    fn turn_ends_trigger_the_token_ledger_scan() {
        let ev = |turn: Option<TurnEnded>| RowEvent {
            session_id: "not-a-live-session".into(),
            agent_name: "tab-x".into(),
            workspace_path: Some("/w/one".into()),
            row: Some(json!({})),
            turn_ended: turn,
            holds_awake: false,
            evidence_at: None,
        };
        assert_eq!(idle_scan_cwd(&ev(None)), None);
        let end = TurnEnded {
            outcome: k2_core::activity::Outcome::Success,
            reason: k2_core::activity::Reason::TurnDone,
            at: 1,
        };
        assert_eq!(idle_scan_cwd(&ev(Some(end))).as_deref(), Some("/w/one"));
    }
}
