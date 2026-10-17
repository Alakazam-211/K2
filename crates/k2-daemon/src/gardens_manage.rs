//! "Manage Gardens and data sources" (`gardens_manage`, migration 0181;
//! Rosson 2026-10-15/16: agents run Gardens end to end).
//!
//! A per-agent switch on the Agent tab, written only by
//! `POST /cli/agent-access/set {toggle: "gardens_manage"}` (Owner/Admin;
//! not an agent verb, so an agent never grants it to itself). With it on,
//! the workspace's agent (and a K2 shell tab in that workspace, which
//! follows the agent rules) acts with no person in the loop:
//!
//! - data sources: approve (its own proposals and others' on this server),
//!   bind a secret it may already use (its own workspace's secret, or a
//!   server secret when it also has vault_manage), pause/resume,
//!   pin/unpin, approve/add/pause/resume triggers within the default
//!   cooldown and cap;
//! - Gardens: put a Garden page or a widget file, make a Garden, approve or
//!   reject Garden/widget proposals.
//!
//! The rails stay: the grant itself, the vault's site pinning (a bind is
//! narrowed to the source's one origin and must fit inside the secret's
//! sites), the Garden Lock, and these audit lines. Every act under the
//! switch writes one Access Audit line (Settings → Logs → Access Audit):
//! who (`agent:<handle>`), what, the target, the secret's NAME (never its
//! value) and the session.

use crate::compute_routes::Who;
use crate::sidecar_routes::Caller;

/// Does `project_id`'s agent have the switch? Unknown workspace → false.
pub fn workspace_allows(project_id: &str) -> bool {
    k2_core::workspace::settings::agents_can_manage_gardens(project_id)
}

/// A sources caller (agent or K2 shell passport) whose workspace has the
/// switch. People never need it; app passes never get it.
pub fn who_may(who: &Who) -> bool {
    match who {
        Who::Agent { project_id, .. } | Who::Shell { project_id, .. } => workspace_allows(project_id),
        _ => false,
    }
}

/// A Zen caller (local agent or K2 shell passport) whose workspace has the
/// switch. A sandboxed agent never does: Zen is this computer's.
pub fn caller_may(caller: &Caller) -> bool {
    match caller {
        Caller::Agent(a) if !a.sandboxed => workspace_allows(&a.project_id),
        Caller::Shell { project_id } => workspace_allows(project_id),
        _ => false,
    }
}

/// The passport's session id, for the audit line.
pub fn session_of(who: &Who) -> Option<&str> {
    match who {
        Who::Agent { session_id, .. } | Who::Shell { session_id, .. } => Some(session_id),
        _ => None,
    }
}

/// The Access Audit outcome for one act: `ok target=<t> secret=<NAME>
/// session=<id>` (the secret's name only, never its value).
pub fn outcome(target: &str, secret: Option<&str>, session: Option<&str>) -> String {
    let mut s = format!("ok target={target}");
    if let Some(n) = secret {
        s.push_str(&format!(" secret={n}"));
    }
    if let Some(id) = session {
        s.push_str(&format!(" session={id}"));
    }
    s
}

/// One Access Audit line (`~/.k2/auth-audit.jsonl`) for an act an agent
/// did under the switch. `event` names the act (`sources-approve`,
/// `sources-bind`, `zen-garden-put`, …); `user` is `agent:<handle>` or
/// `shell:<handle>`.
pub fn audit(event: &str, user: &str, target: &str, secret: Option<&str>, session: Option<&str>) {
    k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(
        event,
        user,
        outcome(target, secret, session),
        "-",
        "-",
        "cli",
    ));
}

/// [`audit`] for a sources caller; a person's act is not logged here (the
/// sources audit has it).
pub fn audit_who(who: &Who, event: &str, target: &str, secret: Option<&str>) {
    if matches!(who, Who::Agent { .. } | Who::Shell { .. }) {
        audit(event, &who.label(), target, secret, session_of(who));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outcome_names_the_secret_never_a_value_and_the_session() {
        assert_eq!(outcome("fleet", Some("VERCEL_RO"), Some("s-1")), "ok target=fleet secret=VERCEL_RO session=s-1");
        assert_eq!(outcome("g-1", None, None), "ok target=g-1");
    }

    #[test]
    fn people_and_apps_never_hold_the_switch() {
        let _home = crate::test_support::TempHome::new();
        let _db = k2_core::db::scoped_for_test();
        for w in [Who::Owner, Who::Wrong, Who::Invalid] {
            assert!(!who_may(&w), "{w:?}");
        }
        assert!(!who_may(&Who::Agent { project_id: "no-such-ws".into(), session_id: "s".into() }), "unknown workspace fails closed");
        assert!(!caller_may(&Caller::Owner));
    }
}
