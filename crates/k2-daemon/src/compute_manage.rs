//! "Allow managing compute nodes" (`compute_manage`, migration 0194;
//! Rosson 2026-10-18: a toggle the owner grants is real end-to-end
//! authority for agents).
//!
//! A per-agent switch on the Agent tab, written only by
//! `POST /cli/agent-access/set {toggle: "compute_manage"}` (the owner
//! only: an Admin login can't, because the switch hands out owner verbs;
//! never an agent verb, so an agent can't grant it to itself or anyone).
//! With it on, a **local** agent passport from that workspace (and a K2
//! shell tab in it, which follows the agent rules) may run, with no person
//! in the loop:
//!
//! - `k2 compute node add` (mint an enroll code) and `node confirm`;
//! - `node rename`, `node pause|drain|resume`, `node retire`,
//!   `node route add|rm`;
//! - `k2 compute grant` / `revoke` for any workspace;
//! - and it reads the full node and grant lists (no SAS: the 6-digit check
//!   is still read off the node itself).
//!
//! Still the owner's: `k2 compute preview on|off`, `node remove` (hard
//! delete) and the switch itself. A passport that came in from anywhere
//! but the loopback listener (LAN, the Connect tunnel, an App link, a
//! loopback proxy relaying someone else), a sandboxed agent, an API cell
//! and every federated or Connect caller never get it: they answer the
//! same `owner_only` as with the switch off.
//!
//! Every act under the switch writes one Access Audit line (Settings →
//! Logs → Access Audit): who (`agent:<handle>`), the act, the node, the
//! workspace and the session. `node add` and `node confirm` also drop an
//! inbox note for the owner ("Agent <ws> added compute node <name>") in
//! the compute "alerts go to" workspace, or the agent's own workspace when
//! none is set, plus a `computeNotice` settings event (like the vault's
//! owner notice).

use crate::compute_routes::Who;

/// A local agent (or K2 shell tab) passport acting under the switch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manager {
    pub project_id: String,
    pub session_id: String,
    /// `agent:<handle>` / `shell:<handle>`.
    pub label: String,
    /// The workspace's address name.
    pub handle: String,
}

/// Does `project_id`'s agent have the switch? Unknown workspace → false.
pub fn workspace_allows(project_id: &str) -> bool {
    k2_core::workspace::settings::agents_can_manage_compute(project_id)
}

/// Is the passport's live session a sandboxed (microVM) agent? Those never
/// manage this server's machines. A session that's gone is not sandboxed
/// (the passport still names its workspace, as everywhere else).
fn sandboxed(session_id: &str) -> bool {
    crate::v2_session_map::snapshot()
        .into_iter()
        .find(|(_, s)| s.session_id.to_string() == session_id)
        .is_some_and(|(_, s)| matches!(s.sandbox, k2_core::terminal::SandboxSpec::Microvm))
}

/// The manager behind `who`, if any: a passport (agent or K2 shell tab)
/// that reached the loopback listener (`local`), is not sandboxed, and
/// whose workspace has the switch on. Everyone else → `None` (people
/// keep their own owner/admin rules; the rest stay `owner_only`).
pub fn manager(who: &Who, local: bool) -> Option<Manager> {
    if !local {
        return None;
    }
    let (project_id, session_id) = match who {
        Who::Agent { project_id, session_id } | Who::Shell { project_id, session_id } => (project_id, session_id),
        _ => return None,
    };
    if !workspace_allows(project_id) || sandboxed(session_id) {
        return None;
    }
    Some(Manager {
        project_id: project_id.clone(),
        session_id: session_id.clone(),
        label: who.label(),
        handle: crate::compute_routes::ws_handle(project_id),
    })
}

/// The Access Audit outcome: `ok <act> node=<n> workspace=<handle>
/// session=<id>` plus any extra `key=value` facts.
pub fn outcome(m: &Manager, act: &str, node: &str, extra: &[(&str, &str)]) -> String {
    let mut s = format!("ok {act} node={node} workspace={} session={}", m.handle, m.session_id);
    for (k, v) in extra {
        s.push_str(&format!(" {k}={v}"));
    }
    s
}

/// One Access Audit line (`~/.k2/auth-audit.jsonl`) for an act an agent
/// did under the switch. `event` is `compute-<act>` (`compute-node-add`,
/// `compute-grant`, …).
pub fn audit(m: &Manager, act: &str, node: &str, extra: &[(&str, &str)]) {
    k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(
        &format!("compute-{act}"),
        &m.label,
        outcome(m, act, node, extra),
        "loopback",
        "-",
        "cli",
    ));
}

/// Where the owner notice goes: the compute "alerts go to" workspace, else
/// the agent's own workspace.
fn notice_workspace(m: &Manager) -> Option<String> {
    let to = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::compute::admin::setting(&conn, k2_core::compute::admin::ALERTS_TO)
    };
    let id = to.unwrap_or_else(|| m.project_id.clone());
    crate::compute_routes::ws_by_id(&id).map(|w| w.path)
}

/// The owner notice for `node add` / `node confirm`: one inbox note plus a
/// `computeNotice` settings event. `verb` is `added` or `confirmed`.
pub fn owner_notice(m: &Manager, verb: &str, node: &str) {
    let title = format!("Agent {} {verb} compute node {node}", m.handle);
    let undo = format!("k2 compute node remove {node}");
    let text = format!(
        "{title}.\n\nIt acted under \"Allow managing compute nodes\" (Settings → Workspaces → {} → Agent), session {}.\n\
         Every act is in Settings → Logs → Access Audit. To undo, from a terminal outside K2: {undo}\n\
         To stop it: turn the switch off, or k2 compute manage off --workspace {}",
        m.handle, m.session_id, m.handle
    );
    if let Some(path) = notice_workspace(m) {
        if let Err(e) =
            k2_core::inbox::compose(std::path::Path::new(&path), &title, &text, Some("normal"), Some("compute"), Some("compute"))
        {
            k2_core::log_debug!("[compute] manage notice for {node}: {e}");
        }
    }
    k2_core::agent_hooks::emit(
        k2_core::agent_hooks::HookEvent::SyncSettings,
        serde_json::json!({ "computeNotice": {
            "kind": format!("node-{verb}"),
            "by": m.label,
            "workspace": m.handle,
            "session": m.session_id,
            "node": node,
            "undo": undo,
        } }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m() -> Manager {
        Manager { project_id: "p".into(), session_id: "s-1".into(), label: "agent:ops".into(), handle: "ops".into() }
    }

    #[test]
    fn outcome_names_the_act_node_workspace_and_session() {
        assert_eq!(outcome(&m(), "node-add", "mini-1", &[]), "ok node-add node=mini-1 workspace=ops session=s-1");
        assert_eq!(
            outcome(&m(), "grant", "mini-1", &[("for", "web")]),
            "ok grant node=mini-1 workspace=ops session=s-1 for=web"
        );
    }

    #[test]
    fn people_apps_remote_and_unknown_workspaces_never_manage() {
        let _home = crate::test_support::TempHome::new();
        let _db = k2_core::db::scoped_for_test();
        for w in [Who::Owner, Who::Wrong, Who::Invalid] {
            assert_eq!(manager(&w, true), None, "{w:?}");
        }
        let agent = Who::Agent { project_id: "no-such-ws".into(), session_id: "s".into() };
        assert_eq!(manager(&agent, true), None, "unknown workspace fails closed");
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO projects (id, name, path, handle, agents_can_manage_compute) VALUES ('w-cm', 'cm', '/tmp/w-cm', 'cm', 1)",
                [],
            )
            .unwrap();
        }
        let agent = Who::Agent { project_id: "w-cm".into(), session_id: "s".into() };
        assert_eq!(manager(&agent, false), None, "never from off the loopback listener");
        let got = manager(&agent, true).expect("a local passport with the switch manages");
        assert_eq!((got.project_id.as_str(), got.handle.as_str(), got.label.as_str()), ("w-cm", "cm", "agent:cm"));
        let shell = Who::Shell { project_id: "w-cm".into(), session_id: "s2".into() };
        assert!(manager(&shell, true).is_some(), "a K2 shell tab follows the agent rules");
    }
}
