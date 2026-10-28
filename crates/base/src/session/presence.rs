//! Presence shared by every session kind (text sessions and tier-0 blob sessions): the
//! per-connection core, participant checks, rights refusals, idle expiry and the roster
//! fold (one entry per pid; role, away and hold from its connections).
//!
//! A session's own connection type wraps a [`Presence`] and derefs to it, adding what is
//! kind-specific (a text caret and view; a blob view region).

use std::ops::Deref;

use super::config::Config;
use super::error::{Result, SessionError};
use super::host::{Need, Rights};
use super::participant::{ConnId, Participant, Role, RosterEntry, Stay, RESERVED_PID_PREFIX};

/// One connection's presence, whatever the doc kind.
pub(crate) struct Presence {
    pub p: Participant,
    pub conn: ConnId,
    /// Reusable by `join_shared`.
    pub shared: bool,
    pub rights: Rights,
    pub stay: Stay,
    /// Join order within the session.
    pub joined: u64,
    pub last_active_ms: u64,
    pub last_edit_ms: Option<u64>,
    /// Latest caret move (text) or other sign of editing intent.
    pub last_cursor_ms: Option<u64>,
    pub hidden_since_ms: Option<u64>,
}

impl Presence {
    pub fn new(
        p: Participant,
        shared: bool,
        rights: Rights,
        stay: Stay,
        joined: u64,
        now: u64,
    ) -> Self {
        Self {
            p,
            conn: ConnId::fresh(),
            shared,
            rights,
            stay,
            joined,
            last_active_ms: now,
            last_edit_ms: None,
            last_cursor_ms: None,
            hidden_since_ms: None,
        }
    }

    /// `set_visible`: a connection is away once hidden for [`Config::away_after_ms`].
    pub fn set_visible(&mut self, visible: bool, now: u64) {
        if visible {
            self.hidden_since_ms = None;
        } else if self.hidden_since_ms.is_none() {
            self.hidden_since_ms = Some(now);
        }
    }

    /// Without a hold and idle for [`Config::idle_leave_ms`].
    pub fn is_idle(&self, now: u64, c: &Config) -> bool {
        self.stay == Stay::UntilIdle && now.saturating_sub(self.last_active_ms) >= c.idle_leave_ms
    }

    /// Whether `join_shared` with `p` may reuse this connection.
    pub fn shares_with(&self, p: &Participant) -> bool {
        self.shared
            && self.p.pid == p.pid
            && self.p.via == p.via
            && self.p.on_behalf_of == p.on_behalf_of
    }
}

/// A host-stamped participant the kit accepts: a non-empty, non-reserved pid, and a valid
/// `on_behalf_of`.
pub(crate) fn check_participant(p: &Participant) -> Result<()> {
    if p.pid.is_empty() || p.pid.starts_with(RESERVED_PID_PREFIX) {
        return Err(SessionError::BadParticipant(format!(
            "pid {:?} is empty or reserved ({RESERVED_PID_PREFIX}*)",
            p.pid
        )));
    }
    if let Some(o) = &p.on_behalf_of {
        if o.is_empty() || o.starts_with(RESERVED_PID_PREFIX) || *o == p.pid {
            return Err(SessionError::BadParticipant(format!(
                "on_behalf_of {o:?} is empty, reserved, or the participant itself"
            )));
        }
    }
    Ok(())
}

/// The right `rights` lacks for a call needing `need`, if any. Every call needs read.
pub(crate) fn refusal(rights: Rights, need: Need) -> Option<Need> {
    if !rights.read {
        Some(Need::Read)
    } else if need == Need::Edit && !rights.edit {
        Some(Need::Edit)
    } else if need == Need::History && !rights.history {
        Some(Need::History)
    } else if need == Need::Owner && !rights.owner {
        Some(Need::Owner)
    } else {
        None
    }
}

/// Distinct pids among the connections.
pub(crate) fn participants<M: Deref<Target = Presence>>(members: &[M]) -> usize {
    let mut p: Vec<&str> = members.iter().map(|m| m.p.pid.as_str()).collect();
    p.sort_unstable();
    p.dedup();
    p.len()
}

/// The roster fold: one row per pid, its connections most active first (latest edit or
/// caret move, then latest call, then newest), with the kind-independent fields filled.
/// `editing(pid)` forces the editor role for a pid with edit rights (a lease holder). The
/// caller fills the kind's own fields from the connections, then the rows are sorted by
/// [`sort_roster`].
pub(crate) fn fold_roster<'a, M: Deref<Target = Presence>>(
    members: &'a [M],
    now: u64,
    c: &Config,
    editing: impl Fn(&str) -> bool,
) -> Vec<(Vec<&'a M>, RosterEntry, u64)> {
    let recent = |t: Option<u64>| t.is_some_and(|t| now.saturating_sub(t) < c.editor_window_ms);
    // members are in join order, so pids come out in order of first join
    let mut pids: Vec<&str> = Vec::new();
    for m in members {
        if !pids.contains(&m.p.pid.as_str()) {
            pids.push(&m.p.pid);
        }
    }
    pids.into_iter()
        .map(|pid| {
            let mut conns: Vec<&M> = members.iter().filter(|m| m.p.pid == pid).collect();
            conns.sort_by_key(|m| {
                std::cmp::Reverse((
                    m.last_edit_ms.max(m.last_cursor_ms),
                    m.last_active_ms,
                    m.joined,
                ))
            });
            let top = conns[0];
            let role = if top.rights.edit
                && (recent(top.last_edit_ms) || recent(top.last_cursor_ms) || editing(pid))
            {
                Role::Editor
            } else {
                Role::Viewer
            };
            let away = conns.iter().all(|m| {
                m.hidden_since_ms
                    .is_some_and(|t| now.saturating_sub(t) >= c.away_after_ms)
            });
            let first_joined = conns.iter().map(|m| m.joined).min().unwrap_or(0);
            let stay = if conns.iter().any(|m| m.stay == Stay::Held) {
                Stay::Held
            } else {
                Stay::UntilIdle
            };
            let entry = RosterEntry {
                participant: top.p.clone(),
                role,
                away,
                cursor: None,
                view: None,
                last_edit_ms: conns.iter().filter_map(|m| m.last_edit_ms).max(),
                stay,
                connections: conns.len(),
                region: None,
                holds_lease: false,
                claims: Vec::new(),
                last_edit: None,
            };
            (conns, entry, first_joined)
        })
        .collect()
}

/// Editors first, then viewers, then away; by first join within each.
pub(crate) fn sort_roster<X>(mut rows: Vec<(X, RosterEntry, u64)>) -> Vec<RosterEntry> {
    rows.sort_by_key(|(_, e, joined)| (e.away, e.role, *joined));
    rows.into_iter().map(|r| r.1).collect()
}
