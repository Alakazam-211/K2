//! K2 as the multiplayer kit's [`Host`]: rights per participant, the clock,
//! and the audit sink (Markdown multiplayer v1, K2).
//!
//! **Rights come from the pid alone.** The kit asks [`Host::rights`] at join,
//! on every call (read) and on every edit (edit), with the session locked.
//! K2 stamps every participant itself ([`super::caller`]), so the pid names
//! who it is, and this module re-derives that principal's rights from K2's
//! current state on each call: a role change, a removed connection or a
//! revoked share takes effect on the next call, and on the 2 s timer for
//! open connections ([`super::maintain`]).
//!
//! | pid | Who | Rights |
//! |---|---|---|
//! | `person:owner` | the owner token, a shell tab (via `terminal`), a login the owner linked | all, any workspace |
//! | `person:<login>` | a Connect login | Owner/Admin: all; Member: read + edit + history |
//! | `agent:<project id>` | an agent's passport (Loopback / its cell socket) | its own workspace: all. Another workspace: needs a connection **and** a share (MI25), see [`agent_rights`] |
//!
//! History (`log`, versions) is a third right K2 checks itself until the
//! kit's `Rights` carries it (K2 Cores' amendment).
//!
//! **Called under a session lock**: never call into the kit from here, and
//! take K2's DB lock only briefly (the DB lock is never held while calling
//! the kit anywhere in `docs`, so the order is always session, then DB).
//!
//! Audit: refusals and kicks go to the Access Audit (`auth_audit`) as
//! metadata only (doc key, pid, action); joins and leaves to the debug log.

use std::time::{SystemTime, UNIX_EPOCH};

use k2_core::log_debug;
use k2_multiplayer_base::session::{AuditEvent, Host, LeaveReason, Participant, Rights};
use k2_multiplayer_base::store::DocKey;

/// pid prefix for people.
pub const PERSON_PREFIX: &str = "person:";
/// pid prefix for agents (followed by the workspace's project id).
pub const AGENT_PREFIX: &str = "agent:";

/// Access Audit event names (Settings → Access Audit).
pub const AUDIT_REFUSED: &str = "doc-refused";
pub const AUDIT_REMOVED: &str = "doc-removed";

/// What one principal may do with one doc. `history` is K2's until the kit
/// carries it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DocRights {
    pub read: bool,
    pub edit: bool,
    pub history: bool,
}

impl DocRights {
    pub const NONE: DocRights = DocRights { read: false, edit: false, history: false };
    pub const FULL: DocRights = DocRights { read: true, edit: true, history: true };

    /// The kit's view (read, edit).
    pub fn kit(self) -> Rights {
        match (self.read, self.edit) {
            (true, true) => Rights::EDIT,
            (true, false) => Rights::READ,
            _ => Rights::NONE,
        }
    }

    /// Wire shape: `{read, edit, history}`.
    pub fn json(self) -> serde_json::Value {
        serde_json::json!({ "read": self.read, "edit": self.edit, "history": self.history })
    }
}

/// Why a cross-workspace agent has no rights (for the refusal it gets).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CrossRefusal {
    /// No comms connection between the two workspaces (taught as
    /// `k2 connections add`, `walls.rs`).
    NotConnected,
    /// Connected, but the owner hasn't shared this file or folder (MI25).
    NotShared,
}

/// A parsed pid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Principal {
    /// `person:<person key>`: `owner` or a Connect login.
    Person(String),
    /// `agent:<project id>`.
    Agent(String),
}

impl Principal {
    pub fn parse(pid: &str) -> Option<Principal> {
        if let Some(k) = pid.strip_prefix(PERSON_PREFIX).filter(|k| !k.is_empty()) {
            return Some(Principal::Person(k.to_string()));
        }
        if let Some(id) = pid.strip_prefix(AGENT_PREFIX).filter(|k| !k.is_empty()) {
            return Some(Principal::Agent(id.to_string()));
        }
        None
    }

    pub fn pid(&self) -> String {
        match self {
            Principal::Person(k) => format!("{PERSON_PREFIX}{k}"),
            Principal::Agent(id) => format!("{AGENT_PREFIX}{id}"),
        }
    }
}

/// A person's rights on any doc on this box. The owner (and the logins the
/// owner linked, which resolve to `owner`) may do everything; a login by
/// its current role. A login that no longer exists gets nothing.
pub fn person_rights(person_key: &str) -> DocRights {
    if person_key == k2_core::notify_prefs::OWNER {
        return DocRights::FULL;
    }
    match k2_core::connect_users::role_for_user(person_key) {
        // Every login role is Member or above (the Viewer role is gone):
        // Member reads and edits files today (`/cli/fs/write-file` is a
        // Member route), and editors get history (K-PERM-9).
        Some(_) => DocRights::FULL,
        None => DocRights::NONE,
    }
}

/// An agent's rights on a doc in workspace `space`. Its own workspace: all.
/// Another workspace: a comms connection is required but not enough; the
/// owner must have shared the file or a folder above it (MI25, K-PERM-4).
/// Shares land with the kit's host table (K2 Cores' amendment); until then
/// every cross-workspace doc is `NotShared`.
pub fn agent_rights(agent_project: &str, space: &str, _rel: &str) -> Result<DocRights, CrossRefusal> {
    if agent_project == space {
        return Ok(DocRights::FULL);
    }
    if !k2_core::connections::are_local_peers(agent_project, space) {
        return Err(CrossRefusal::NotConnected);
    }
    Err(CrossRefusal::NotShared)
}

/// Rights for `pid` on `key` (the whole rule, used by the kit and by K2's
/// own history check).
pub fn rights_for(pid: &str, key: &DocKey) -> DocRights {
    match Principal::parse(pid) {
        Some(Principal::Person(k)) => person_rights(&k),
        Some(Principal::Agent(id)) => agent_rights(&id, &key.space, &key.path).unwrap_or(DocRights::NONE),
        None => DocRights::NONE,
    }
}

/// The host. Stateless: everything is derived from the pid and K2's state.
#[derive(Debug, Default)]
pub struct K2Host;

impl Host for K2Host {
    fn rights(&self, who: &Participant, key: &DocKey) -> Rights {
        rights_for(&who.pid, key).kit()
    }

    fn now_ms(&self) -> u64 {
        SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
    }

    fn on_event(&self, ev: &AuditEvent) {
        match ev {
            AuditEvent::Refused { key, pid, action, need } => audit(
                AUDIT_REFUSED,
                pid,
                format!("{} action={action:?} need={need:?}", key_label(key)),
            ),
            AuditEvent::Left { key, pid, conn, reason } => match reason {
                LeaveReason::Kicked | LeaveReason::RightsLost => audit(
                    AUDIT_REMOVED,
                    pid,
                    format!("{} {conn} reason={reason:?}", key_label(key)),
                ),
                LeaveReason::Left | LeaveReason::Idle => {
                    log_debug!("[docs] {pid} left {} ({conn}, {reason:?})", key_label(key))
                }
            },
            AuditEvent::Joined { key, pid, conn, .. } => {
                log_debug!("[docs] {pid} joined {} ({conn})", key_label(key))
            }
            AuditEvent::CompactFailed { key, error } => {
                log_debug!("[docs] WARN: compaction failed for {}: {error}", key_label(key))
            }
            AuditEvent::Recovered { key, error } => {
                log_debug!("[docs] WARN: store write failed for {}; session reloaded: {error}", key_label(key))
            }
            other => log_debug!("[docs] event {other:?}"),
        }
    }
}

/// `<space>:<path>`: metadata only.
pub fn key_label(key: &DocKey) -> String {
    format!("{}:{}", key.space, key.path)
}

/// One Access Audit line (metadata only, never doc content).
pub fn audit(event: &str, pid: &str, outcome: String) {
    k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(event, pid, outcome, "-", "-", "cli"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pids_round_trip_and_reject_junk() {
        for p in [Principal::Person("owner".into()), Principal::Agent("abc-123".into())] {
            assert_eq!(Principal::parse(&p.pid()), Some(p));
        }
        for junk in ["", "person:", "agent:", "kit:import", "owner", "user:bob"] {
            assert_eq!(Principal::parse(junk), None, "{junk:?}");
        }
    }

    #[test]
    fn unknown_pids_get_nothing() {
        let key = DocKey { space: "ws".into(), path: "a.md".into() };
        assert_eq!(rights_for("user:bob", &key), DocRights::NONE);
        assert_eq!(rights_for("kit:import", &key), DocRights::NONE);
    }

    #[test]
    fn the_owner_has_every_right_and_an_agent_has_its_own_workspace() {
        let key = DocKey { space: "ws-a".into(), path: "a.md".into() };
        assert_eq!(rights_for("person:owner", &key), DocRights::FULL);
        assert_eq!(rights_for("agent:ws-a", &key), DocRights::FULL);
    }

    #[test]
    fn kit_rights_follow_read_and_edit() {
        assert_eq!(DocRights::FULL.kit(), Rights::EDIT);
        assert_eq!(DocRights { read: true, edit: false, history: true }.kit(), Rights::READ);
        assert_eq!(DocRights { read: false, edit: true, history: false }.kit(), Rights::NONE);
    }
}
