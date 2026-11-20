//! A heartbeat that is turned off, or paused on the LLM usage limit, says
//! so (0.46.18, field report: two daily heartbeats were auto-disabled
//! after 5 fires that failed on a weekly limit; nothing told the agent,
//! its manager or the owner, and the agent went quiet for 12 days).
//!
//! Two paths, both from `heartbeat_launch::record_fire_failure`:
//! - **Usage limit → pause** ([`usage_cause`], [`pause`]): the failure's
//!   reason is the limit (`usage_pause::cause_from_reason`), or the
//!   out-of-usage signal (`llm_out_notice`, [`note_out`]) saw this
//!   workspace's login run out. The row is paused until the reset (the
//!   signal's, else the login's reading, else an hourly re-check), the
//!   failure doesn't count toward the 5, and the first pause posts one
//!   notice on the agent's Thread.
//! - **Auto-disable → notice** ([`notify_disabled`]): the 5th failure that
//!   isn't the limit turns the row off as before, and posts one notice
//!   with a Turn it back on button (the heartbeat's own enable route).
//!
//! Both emit `heartbeat_turned_off` on the session bus (APP-LEVEL), the
//! one channel the agent's manager and connected views already read.
//! The tick re-checks pauses whose time came ([`sweep`]).

use std::collections::HashMap;
use std::sync::OnceLock;

use chrono::FixedOffset;
use parking_lot::Mutex;

use k2_core::db::schema::AgentHeartbeat;
use k2_core::heartbeats::usage_pause::{self as up, Cause};
use k2_core::llm_accounts::out_of_usage::{self as ou, OutKind};
use k2_core::overlay::SystemNotice;

use crate::llm_out_notice::{LiveWorld, Place, World};

/// One turned-off or paused heartbeat (the session event's facts).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnedOff {
    pub workspace_id: String,
    pub address: String,
    pub name: String,
    /// [`STATE_DISABLED`] | [`STATE_PAUSED`].
    pub state: &'static str,
    pub reason: String,
    pub failures: i64,
    pub paused_until: Option<i64>,
    pub notice_id: Option<String>,
}

pub const STATE_DISABLED: &str = "disabled";
pub const STATE_PAUSED: &str = "paused";

/// What this module needs from the daemon (faked in tests).
pub trait Effects {
    /// The agent's Thread address for a workspace.
    fn addr(&self, project_id: &str) -> String;
    /// Post the daemon's line on `addr`'s Thread; the doc id.
    fn post(&self, addr: &str, text: &str, notice: SystemNotice) -> Result<String, String>;
    fn emit(&self, t: &TurnedOff);
    /// The server's UTC offset at `at` (the reset's words).
    fn offset(&self, at: i64) -> FixedOffset;
    /// The reset of the workspace's login reading, when the reading says
    /// it is at its limit now.
    fn reading_reset(&self, project_id: &str, addr: &str, now: i64) -> Option<i64>;
}

// ── The out-of-usage signal, per workspace ──────────────────────────

#[derive(Debug, Clone, Copy)]
struct Out {
    kind: OutKind,
    resets_at: Option<i64>,
    at: i64,
}

fn outs() -> &'static Mutex<HashMap<String, Out>> {
    static O: OnceLock<Mutex<HashMap<String, Out>>> = OnceLock::new();
    O.get_or_init(|| Mutex::new(HashMap::new()))
}

/// `llm_out_notice` saw this workspace's login run out (every resolved
/// signal, before its one-notice gate).
pub fn note_out(workspace_id: &str, kind: OutKind, resets_at: Option<i64>, now: i64) {
    let mut m = outs().lock();
    m.retain(|_, o| still_out(o, now));
    m.insert(workspace_id.to_string(), Out { kind, resets_at, at: now });
}

/// A signal is still news: its reset is ahead, or (no reset) it is
/// recent.
fn still_out(o: &Out, now: i64) -> bool {
    match o.resets_at {
        Some(r) => r > now,
        None => now - o.at <= ou::MAX_RECORD_AGE_SECS,
    }
}

fn recent_out(workspace_id: &str, now: i64) -> Option<Out> {
    outs().lock().get(workspace_id).copied().filter(|o| still_out(o, now))
}

/// Forget a workspace's signal (tests).
#[cfg(test)]
pub(crate) fn forget_out(workspace_id: &str) {
    outs().lock().remove(workspace_id);
}

/// Was this failed fire the usage limit? The reason says so, or the
/// out-of-usage signal saw the workspace's login run out.
pub fn usage_cause(project_id: &str, reason: &str, now: i64) -> Option<Cause> {
    let hit = recent_out(project_id, now);
    match up::cause_from_reason(reason) {
        Some(mut c) => {
            if c.resets_at.is_none() {
                c.resets_at = hit.and_then(|h| h.resets_at);
            }
            Some(c)
        }
        None => hit.map(|h| Cause { kind: h.kind, resets_at: h.resets_at }),
    }
}

// ── Pause ───────────────────────────────────────────────────────────

/// A pause that was written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paused {
    /// When it may fire again (unix seconds).
    pub until: i64,
    /// The reset people are told, when known.
    pub reset: Option<i64>,
    /// A new pause (it posted the notice), not a re-pause.
    pub new: bool,
    pub notice_id: Option<String>,
}

impl Paused {
    /// The failure's audit words.
    pub fn audit_note(&self) -> String {
        match self.reset {
            Some(_) => format!("usage limit: paused until {}, not counted", up::fmt(self.until)),
            None => format!("usage limit: paused, re-checked at {}, not counted", up::fmt(self.until)),
        }
    }
}

/// Pause `hb` on the usage limit. A new pause posts one notice and emits
/// the event; a re-pause (the mark was already set) only moves the time.
/// `None`: the row is off or gone (nothing written).
pub fn pause(fx: &dyn Effects, project_id: &str, hb: &AgentHeartbeat, cause: Cause, reason: &str, now: i64) -> Option<Paused> {
    let addr = fx.addr(project_id);
    let reset = cause.resets_at.filter(|r| *r > now).or_else(|| match cause.kind {
        OutKind::Usage => fx.reading_reset(project_id, &addr, now),
        OutKind::Credit => None,
    });
    let (until, reset) = up::pause_until(reset, now);
    let new = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        // Only an enabled row is paused: a forced fire of an off row isn't.
        match up::pause(&conn, project_id, &hb.name, until) {
            Ok(Some(new)) => new,
            Ok(None) => return None,
            Err(e) => {
                k2_core::log_debug!("[heartbeat] pause {} failed: {e}", hb.name);
                return None;
            }
        }
    };
    let mut notice_id = None;
    if new {
        let text = up::paused_text(&addr, &hb.name, cause.kind, reset, now, fx.offset(reset.unwrap_or(now)));
        notice_id = post(fx, &addr, &text, up::paused_notice());
        fx.emit(&TurnedOff {
            workspace_id: project_id.to_string(),
            address: addr,
            name: hb.name.clone(),
            state: STATE_PAUSED,
            reason: reason.to_string(),
            failures: hb.consecutive_failures,
            paused_until: Some(until),
            notice_id: notice_id.clone(),
        });
    }
    Some(Paused { until, reset, new, notice_id })
}

fn post(fx: &dyn Effects, addr: &str, text: &str, notice: SystemNotice) -> Option<String> {
    match fx.post(addr, text, notice) {
        Ok(id) => Some(id),
        Err(e) => {
            k2_core::log_debug!("[heartbeat] couldn't tell {addr}'s Thread: {e}");
            None
        }
    }
}

// ── Auto-disable ────────────────────────────────────────────────────

/// `hb` (loaded enabled) was just auto-disabled after `failures` failed
/// fires: one notice with Turn it back on, and the event. A row that was
/// already off (a forced manual fire) gets neither.
pub fn notify_disabled(fx: &dyn Effects, project_id: &str, hb: &AgentHeartbeat, failures: i64, reason: &str) -> Option<String> {
    if !hb.enabled {
        return None;
    }
    let addr = fx.addr(project_id);
    let text = up::disabled_text(&addr, &hb.name, failures, reason);
    let notice_id = post(fx, &addr, &text, up::disabled_notice(&addr, &hb.name));
    fx.emit(&TurnedOff {
        workspace_id: project_id.to_string(),
        address: addr,
        name: hb.name.clone(),
        state: STATE_DISABLED,
        reason: reason.to_string(),
        failures,
        paused_until: None,
        notice_id: notice_id.clone(),
    });
    notice_id
}

// ── The re-check ────────────────────────────────────────────────────

/// What the re-check did to one row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rechecked {
    /// Still out: paused until then.
    Extended { name: String, until: i64 },
    /// Nothing says out: it fires on its schedule again.
    Resumed { name: String },
}

/// Re-check this workspace's pauses whose time came: still out (the
/// signal, or the login's reading) moves the time on; otherwise the row
/// resumes. Run at the top of each tick, before the due scan.
pub fn sweep(fx: &dyn Effects, project_id: &str, now: i64) -> Vec<Rechecked> {
    let names = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        up::due_rechecks(&conn, project_id, now).unwrap_or_default()
    };
    if names.is_empty() {
        return Vec::new();
    }
    let addr = fx.addr(project_id);
    let still = recent_out(project_id, now)
        .map(|o| o.resets_at)
        .or_else(|| fx.reading_reset(project_id, &addr, now).map(Some));
    let mut out = Vec::new();
    for name in names {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let done = match up::recheck(still, now) {
            Some(until) => up::extend(&conn, project_id, &name, until).map(|_| Rechecked::Extended { name, until }),
            None => up::resume(&conn, project_id, &name).map(|_| Rechecked::Resumed { name }),
        };
        match done {
            Ok(r) => out.push(r),
            Err(e) => k2_core::log_debug!("[heartbeat] usage re-check failed: {e}"),
        }
    }
    out
}

/// [`sweep`] for a workspace path (the tick's entry).
pub fn sweep_project(project_path: &str) {
    let project_id = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::workspace::agent_identity::resolve_project_id(&conn, project_path)
    };
    let Some(project_id) = project_id else { return };
    let done = sweep(&LiveEffects, &project_id, chrono::Utc::now().timestamp());
    for r in &done {
        k2_core::log_debug!("[heartbeat] usage re-check {project_path}: {r:?}");
    }
    if !done.is_empty() {
        crate::heartbeat_wait::refresh_and_emit(project_path);
    }
}

// ── The live effects ────────────────────────────────────────────────

/// The daemon's own Thread, bus and wallet.
pub struct LiveEffects;

impl Effects for LiveEffects {
    fn addr(&self, project_id: &str) -> String {
        crate::compute_routes::ws_handle(project_id)
    }

    fn post(&self, addr: &str, text: &str, notice: SystemNotice) -> Result<String, String> {
        crate::overlay_routes::post_workspace_notice(addr, text, notice)
    }

    fn emit(&self, t: &TurnedOff) {
        crate::session_events::emit_heartbeat_turned_off(t);
    }

    fn offset(&self, at: i64) -> FixedOffset {
        LiveWorld.offset(at)
    }

    fn reading_reset(&self, project_id: &str, addr: &str, now: i64) -> Option<i64> {
        let place = Place { addr: addr.to_string(), workspace_id: project_id.to_string() };
        LiveWorld.login(&place, None, None, now).filter(|l| !l.api_key).and_then(|l| l.reading_reset).filter(|r| *r > now)
    }
}

#[cfg(test)]
#[path = "heartbeat_turned_off_tests.rs"]
mod tests;
