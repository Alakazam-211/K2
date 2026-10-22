//! A1 — The outcome turn `[permission:<id>] approved|already on|denied|
//! expired|stale|failed|closed …` into the agent's chat (§3.3). "X is now
//! on; run your command again"; never the agent's command line (C5).
//!
//! Each outcome wakes the agent once, at the address the card was posted
//! at (`permission_requests.addr`). Notes never reach the turn. A request
//! the agent withdrew itself (`cancelled`) sends no turn.
//!
//! Under test nothing is delivered: every line goes to a sink the tests
//! read (no test wakes a real agent, §13.3).

use k2_core::permission::catalog::{Direction, Grant};
use k2_core::permission::request::{limits, PermissionRequest, RequestState};

use super::present;

/// The `[permission:<id>]` prefix the agent sheet teaches.
pub(crate) fn prefix(req: &PermissionRequest) -> String {
    format!("[permission:{}]", req.display_id())
}

fn capitalized(s: &str) -> String {
    let t = s.trim().trim_end_matches('.');
    let mut c = t.chars();
    match c.next() {
        Some(f) => format!("{}{}.", f.to_uppercase(), c.as_str()),
        None => String::new(),
    }
}

/// "14:02" today, "14:02 tomorrow", else "Oct 23 14:02" (local time).
pub(crate) fn when(at: i64, now: i64) -> String {
    use chrono::{Local, TimeZone};
    let (Some(t), Some(n)) = (Local.timestamp_opt(at, 0).single(), Local.timestamp_opt(now, 0).single()) else {
        return at.to_string();
    };
    let days = (t.date_naive() - n.date_naive()).num_days();
    match days {
        0 => t.format("%H:%M").to_string(),
        1 => format!("{} tomorrow", t.format("%H:%M")),
        _ => t.format("%b %-d %H:%M").to_string(),
    }
}

fn span(secs: i64) -> String {
    let hours = (secs + 1800) / 3600;
    if hours >= 48 {
        let d = (secs + 43_200) / 86_400;
        format!("{d} days")
    } else if hours >= 2 {
        format!("{hours} hours")
    } else {
        let m = (secs + 30) / 60;
        format!("{m} minutes")
    }
}

/// The turn for a request's final state, or None when the agent gets no
/// turn (`cancelled`: it withdrew the request itself; open states).
pub(crate) fn outcome_line(req: &PermissionRequest, now: i64) -> Option<String> {
    let p = prefix(req);
    let by = req.answered_by.as_deref().unwrap_or("someone");
    let found = present::find_action(&req.action);
    let ctx = present::ctx_of(req);
    let (setting, scope, duration, retry, dir, grant) = match &found {
        Some(r) => {
            let w = present::words(r.spec, &ctx);
            (w.setting, w.scope, w.duration, r.spec.words.retry.to_string(), r.spec.direction, r.spec.grant)
        }
        None => (req.action.clone(), ctx.workspace_handle.clone(), String::new(), String::new(), Direction::Loosen, Grant::OneTime),
    };
    let on = present::on_word(dir);
    let retry = if retry.trim().is_empty() { String::new() } else { format!(" {}", capitalized(&retry)) };
    let msg = req.result_message.clone().unwrap_or_default();
    let line = match req.state {
        RequestState::Approved => {
            let head = match grant {
                Grant::Standing => format!("{setting} is now {on} for {scope}, {duration}."),
                Grant::OneTime => {
                    let tail = if msg.trim().is_empty() { String::new() } else { format!(" {}", msg.trim()) };
                    format!("once, for {scope}.{tail}")
                }
            };
            format!("{p} approved by {by}: {head}{retry}")
        }
        RequestState::AlreadyDone => {
            format!("{p} already {on}: {setting} was already {on} for {scope}. Nothing was applied.{retry}")
        }
        RequestState::Denied => {
            let reason = match req.reason.as_deref().map(str::trim).filter(|r| !r.is_empty()) {
                Some(r) if r.ends_with(['.', '!', '?']) => format!(": \"{r}\""),
                Some(r) => format!(": \"{r}.\""),
                None => ".".to_string(),
            };
            let until = req.answered_at.unwrap_or(now) + limits::DENIAL_BLOCK_SECS;
            format!("{p} denied by {by}{reason} Asking again is blocked until {}.", when(until, now))
        }
        RequestState::Expired => {
            format!("{p} expired unanswered after {}. Still need it? Ask once more.", span(req.expires_at - req.created_at))
        }
        RequestState::Stale => format!("{p} stale: {}. Nothing was applied.", msg.trim().trim_end_matches('.')),
        RequestState::Failed => format!("{p} failed: {}. Nothing was applied.", msg.trim().trim_end_matches('.')),
        RequestState::Closed => {
            format!("{p} closed: permission cards were turned off on this server. Nothing was applied. Ask your human the usual way.")
        }
        RequestState::Cancelled | RequestState::Pending | RequestState::Applying => return None,
    };
    Some(line)
}

/// Wake the agent once with the request's outcome (its own address).
pub(crate) fn send_outcome(req: &PermissionRequest) {
    let now = k2_core::overlay::now_secs();
    let Some(line) = outcome_line(req, now) else { return };
    let addr = req.addr.clone().unwrap_or_else(|| present::workspace_handle(&req.workspace_id));
    deliver(&addr, &line);
}

#[cfg(test)]
static SINK: std::sync::Mutex<Vec<(String, String)>> = std::sync::Mutex::new(Vec::new());

/// Lines sent to `addr` (tests only).
#[cfg(test)]
pub(crate) fn sent_to(addr: &str) -> Vec<String> {
    SINK.lock().expect("inject sink").iter().filter(|(a, _)| a == addr).map(|(_, l)| l.clone()).collect()
}

#[cfg(test)]
fn deliver(addr: &str, line: &str) {
    SINK.lock().expect("inject sink").push((addr.to_string(), line.to_string()));
}

/// Inject + submit into the agent's live session (waking it when
/// dormant), off the caller's thread: the answer route and the sweep
/// never wait on a wake. `via=thread`: the card is already on the Thread,
/// so no Chatter line. The thread runs in the daemon's runtime: waking a
/// dormant agent spawns its session, which needs one (`crate::rt`).
#[cfg(not(test))]
fn deliver(addr: &str, line: &str) {
    let addr = addr.to_string();
    let line = line.to_string();
    let spawned = crate::rt::spawn("permission-turn", move || {
        let r = crate::workspace_msg::deliver_live_with_via(
            &addr,
            &line,
            "k2",
            "",
            true,
            crate::workspace_msg::DEFAULT_WAKE_TIMEOUT,
            "thread",
        );
        if !r.success {
            k2_core::log_debug!("[permission] outcome turn to {addr} not delivered: {:?}", r.reason);
        }
    });
    if let Err(e) = spawned {
        k2_core::log_debug!("[permission] could not start the outcome turn: {e}");
    }
}
