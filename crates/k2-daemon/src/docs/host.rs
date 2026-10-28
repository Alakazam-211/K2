//! K2 as the multiplayer kit's [`Host`]: rights per participant, the clock,
//! and the audit sink (Markdown multiplayer v1, K2).
//!
//! **Rights come from the pid alone.** The kit asks [`Host::rights`] at join,
//! on every call (read), on every edit (edit) and on history reads
//! (history), with the session locked. K2 stamps every participant itself
//! ([`super::caller`]), so the pid names who it is, and this module
//! re-derives that principal's rights from K2's current state on each call:
//! a removed login, a removed connection or a revoked share takes effect on
//! the next call, and within the 2 s upkeep for open connections.
//!
//! | pid | Who | Rights |
//! |---|---|---|
//! | `person:owner` | the owner token, a shell tab (via `terminal`), a login the owner linked | all, any workspace |
//! | `person:<login>` | a Connect login (every role is Member or above) | all, any workspace |
//! | `agent:<project id>` | an agent's passport (Loopback / its cell socket) | its own workspace: all. Another workspace: a connection **and** a share (MI25), see [`agent_rights`] |
//!
//! Rights are `{read, edit, history}` (kit amendment 1). History is never
//! implied by read or edit (K-PERM-9): people have it, an agent has it in
//! its own workspace, and elsewhere only where a share grants it. Acting on
//! someone's behalf never widens an agent's rights (K-PERM-10): the kit asks
//! about the agent's own pid.
//!
//! **Called under a session lock**: never call into the kit from here, and
//! take K2's DB lock only briefly (the DB lock is never held while calling
//! the kit anywhere in `docs`, so the order is always session, then DB).
//!
//! Audit: refusals, removals and approvals go to the Access Audit
//! (`auth_audit`) as metadata only (doc key, pid, action); joins and leaves
//! to the debug log.

use std::sync::Weak;
use std::time::{SystemTime, UNIX_EPOCH};

use k2_core::log_debug;
use k2_mpc_base::session::{AuditEvent, Host, LeaveReason, Participant, Rights};
use k2_mpc_base::store::{DocKey, DocStore};

/// pid prefix for people.
pub const PERSON_PREFIX: &str = "person:";
/// pid prefix for agents (followed by the workspace's project id).
pub const AGENT_PREFIX: &str = "agent:";

/// Access Audit event names (Settings → Access Audit).
pub const AUDIT_REFUSED: &str = "doc-refused";
pub const AUDIT_REMOVED: &str = "doc-removed";
pub const AUDIT_APPROVAL: &str = "doc-approval";
/// The file under a live doc changed outside K2, went missing, stopped
/// decoding, came back, or failed to read/write.
pub const AUDIT_FILE: &str = "doc-file";
/// The audit `user` for file events (the kit's outside-edit participant).
const EXTERNAL: &str = k2_mpc_base::session::EXTERNAL_PID;

/// What one principal may do with one doc (K2's copy of the kit's `Rights`,
/// with a wire shape).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DocRights {
    pub read: bool,
    pub edit: bool,
    pub history: bool,
}

impl DocRights {
    pub const NONE: DocRights = DocRights { read: false, edit: false, history: false };
    pub const FULL: DocRights = DocRights { read: true, edit: true, history: true };

    /// The kit's view. Nothing without read; edit and history only with it.
    pub fn kit(self) -> Rights {
        let base = match (self.read, self.edit) {
            (true, true) => Rights::editor(),
            (true, false) => Rights::read_only(),
            _ => return Rights::none(),
        };
        base.with_history(self.history)
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
    /// Connected, but nobody shared this file or a folder above it (MI25).
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

/// Whether `person_key` is a person on this box now: the owner, or a
/// Connect login that still exists.
pub fn person_exists(person_key: &str) -> bool {
    person_key == k2_core::notify_prefs::OWNER || k2_core::connect_users::role_for_user(person_key).is_some()
}

/// A person's rights on any doc on this box: the owner (and the logins the
/// owner linked, which resolve to `owner`) and every existing login may do
/// everything (every role is Member or above, and Member writes files
/// today: `/cli/fs/write-file` is a Member route). A login that no longer
/// exists gets nothing.
pub fn person_rights(person_key: &str) -> DocRights {
    if person_exists(person_key) {
        DocRights::FULL
    } else {
        DocRights::NONE
    }
}

/// An agent's rights on a doc in workspace `space`. Its own workspace: all.
/// Another workspace: a comms connection is required but not enough; a
/// person must have shared the file or a folder above it (MI25, K-PERM-4):
/// read, plus write and history only where a share grants them. `store`
/// holds the shares (`None` once released: nothing is shared then).
pub fn agent_rights(agent_project: &str, space: &str, rel: &str, store: Option<&DocStore>) -> Result<DocRights, CrossRefusal> {
    if agent_project == space {
        return Ok(DocRights::FULL);
    }
    if !k2_core::connections::are_local_peers(agent_project, space) {
        return Err(CrossRefusal::NotConnected);
    }
    let granted = store.and_then(|st| match super::shares::grant(st, space, agent_project, rel) {
        Ok(g) => g,
        Err(e) => {
            log_debug!("[docs] WARN: share lookup failed (refusing): {e}");
            None
        }
    });
    match granted {
        Some((write, history)) => Ok(DocRights { read: true, edit: write, history }),
        None => Err(CrossRefusal::NotShared),
    }
}

/// Rights for `pid` on `key`: the whole rule, used by the kit and by K2's
/// own pre-checks.
pub fn rights_for(pid: &str, key: &DocKey, store: Option<&DocStore>) -> DocRights {
    match Principal::parse(pid) {
        Some(Principal::Person(k)) => person_rights(&k),
        Some(Principal::Agent(id)) => agent_rights(&id, &key.space, &key.path, store).unwrap_or(DocRights::NONE),
        None => DocRights::NONE,
    }
}

/// The host. Holds only a weak handle on the store (for shares), so it never
/// keeps the file open past a release.
#[derive(Debug, Default)]
pub struct K2Host {
    store: Weak<DocStore>,
}

impl K2Host {
    pub fn new(store: Weak<DocStore>) -> Self {
        K2Host { store }
    }
}

impl Host for K2Host {
    fn rights(&self, who: &Participant, key: &DocKey) -> Rights {
        let store = self.store.upgrade();
        rights_for(&who.pid, key, store.as_deref()).kit()
    }

    fn now_ms(&self) -> u64 {
        now_ms()
    }

    fn on_event(&self, ev: &AuditEvent) {
        match ev {
            AuditEvent::Refused { key, pid, action, need } => audit(
                AUDIT_REFUSED,
                pid,
                format!("{} action={action:?} need={need:?}", key_label(key)),
            ),
            AuditEvent::Approval { key, seq, pid, state } => audit(
                AUDIT_APPROVAL,
                pid,
                format!("{} change={seq} state={state:?}", key_label(key)),
            ),
            // Disk sync (kit C4): what happened to the file, metadata only.
            AuditEvent::ExternalEdit { key, seq } => {
                audit(AUDIT_FILE, EXTERNAL, format!("{} external_edit change={seq}", key_label(key)))
            }
            AuditEvent::FileMissing { key, unflushed } => {
                audit(AUDIT_FILE, EXTERNAL, format!("{} missing unflushed={unflushed}", key_label(key)))
            }
            AuditEvent::FileDeclined { key, code } => {
                audit(AUDIT_FILE, EXTERNAL, format!("{} declined code={code}", key_label(key)))
            }
            AuditEvent::FileRestored { key } => audit(AUDIT_FILE, EXTERNAL, format!("{} restored", key_label(key))),
            AuditEvent::DiskError { key, error } => {
                log_debug!("[docs] WARN: disk error on {}: {error}", key_label(key));
                audit(AUDIT_FILE, EXTERNAL, format!("{} disk_error", key_label(key)))
            }
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
            // Purge isn't on k2 doc yet; when it runs, the audit gets what was
            // reached (counts, scope), never content.
            AuditEvent::Purged { key, by, scope, counts } => {
                audit("doc-purged", by, format!("{} scope={scope:?} counts={counts:?}", key_label(key)))
            }
            AuditEvent::RecheckFailed { key, error } => {
                log_debug!("[docs] WARN: rights re-check failed on {}: {error}", key_label(key))
            }
            AuditEvent::DocDeleted { key, connections } => {
                audit(AUDIT_FILE, "k2", format!("{} removed connections={}", key_label(key), connections.len()))
            }
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

/// Milliseconds since the epoch (the kit's clock and the share stamps).
pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
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
        assert_eq!(rights_for("user:bob", &key, None), DocRights::NONE);
        assert_eq!(rights_for("kit:import", &key, None), DocRights::NONE);
    }

    #[test]
    fn the_owner_has_every_right_and_an_agent_has_its_own_workspace() {
        let key = DocKey { space: "ws-a".into(), path: "a.md".into() };
        assert_eq!(rights_for("person:owner", &key, None), DocRights::FULL);
        assert_eq!(rights_for("agent:ws-a", &key, None), DocRights::FULL);
    }

    #[test]
    fn kit_rights_follow_read_edit_and_history() {
        assert_eq!(DocRights::FULL.kit(), Rights::full());
        assert_eq!(DocRights { read: true, edit: false, history: true }.kit(), Rights::read_only().with_history(true));
        assert_eq!(DocRights { read: true, edit: true, history: false }.kit(), Rights::editor());
        assert_eq!(DocRights { read: false, edit: true, history: true }.kit(), Rights::none());
    }
}
