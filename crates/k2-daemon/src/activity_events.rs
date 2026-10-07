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

/// One row as the rollup reads it (§7.2 JSON).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowView {
    pub session_id: String,
    pub agent_name: String,
    pub project_id: Option<String>,
    pub workspace_path: Option<String>,
    pub display: Display,
    pub turn_started_at: Option<i64>,
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
        })
    }
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
        match &self.project_id {
            Some(pid) => row.project_id.as_deref() == Some(pid.as_str()),
            None => row.project_id.is_none() && row.workspace_path == self.workspace_path,
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
        })
    }
}

/// RL1: fold the rows `key` owns. A workspace with no rows is idle.
pub fn rollup(key: &WorkspaceKey, rows: &[RowView]) -> Rollup {
    let mut out = Rollup {
        project_id: key.project_id.clone(),
        workspace_path: key.workspace_path.clone(),
        display: Display::Idle,
        counts: DisplayCounts::default(),
        since: None,
    };
    for row in rows.iter().filter(|r| key.matches(r)) {
        if out.workspace_path.is_none() {
            out.workspace_path = row.workspace_path.clone();
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

/// Every workspace that has at least one row, ordered by key.
pub fn rollups(rows: &[RowView]) -> Vec<Rollup> {
    let mut keys: Vec<WorkspaceKey> = Vec::new();
    for row in rows {
        let Some(k) = WorkspaceKey::of(row) else { continue };
        // Rows of one project may carry different paths; the project id
        // is the key.
        if !keys.iter().any(|have| have.matches(row)) {
            keys.push(k);
        }
    }
    keys.sort();
    keys.iter().map(|k| rollup(k, rows)).collect()
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
        let workspace = key.as_ref().map(|k| rollup(k, &all_views()).to_json());
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
        let workspace = key.map(|k| rollup(&k, &all_views()).to_json());
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
    let p = publisher().lock();
    let mut rows: Vec<Value> = activity_store::rows_json();
    let mut views: Vec<RowView> = rows.iter().filter_map(RowView::from_json).collect();
    if let Some(key) = only {
        rows.retain(|r| RowView::from_json(r).is_some_and(|v| key.matches(&v)));
        views.retain(|v| key.matches(v));
    }
    rows.sort_by(|a, b| a["sessionId"].as_str().cmp(&b["sessionId"].as_str()));
    let workspaces: Vec<Value> = match only {
        Some(key) => vec![rollup(key, &views).to_json()],
        None => rollups(&views).iter().map(Rollup::to_json).collect(),
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
            let next = publisher().lock().flush_due(now);
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
        let all = rollups(&rows);
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
                assert_eq!(rollup(&key, &rows).display, *hi, "{lo:?} vs {hi:?}");
            }
        }
        // A workspace with no rows is idle with zero counts.
        let empty = rollup(&WorkspaceKey { project_id: Some("none".into()), workspace_path: None }, &rows);
        assert_eq!((empty.display, empty.counts, empty.since), (Idle, DisplayCounts::default(), None));
        let j = p1.to_json();
        assert_eq!(j["display"], "working");
        assert_eq!(j["counts"]["working"], 2);
        assert_eq!(j["since"], 300);
        assert_eq!(j["projectId"], "p1");
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
