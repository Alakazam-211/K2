//! One runner per conversation (prd-workspace-sessions-cli-v1 WS-O13, G4).
//!
//! [`conversation_owner`] is the one question every door asks before it
//! starts a harness on a conversation id: who holds it now?
//!
//! - the workspace's pinned chat (`workspace_sessions.session_id`, "main");
//! - a live runner in the session map whose argv resumes it;
//! - an asleep tab row (`workspace_tab_sessions.session_id`).
//!
//! The doors:
//! - `POST /cli/sessions/v2/spawn` (a tab, Chat history's resume tab):
//!   `v2_spawn` turns a [`DuplicateRefusal`] into its 409.
//! - Every in-process `tab-*` spawn (`k2 sidecar new`, `k2 sessions open`,
//!   the `k2 msg` sidecar wake): [`crate::spawn::spawn_agent_session_v2_blocking`]
//!   refuses with [`SPAWN_REFUSAL_PREFIX`].
//! - `k2 sessions open` and `k2 msg --session` read the owner first and
//!   answer with its address instead of spawning.
//!
//! The streamed-sessions runner registry (C1, 0.46.8) later replaces the
//! argv scan behind this same signature (RR-5).

use k2_core::log_debug;

use crate::v2_session_map;

/// What holds a conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Owner {
    /// `main` | `tab` | `heartbeat` | `api` | `other`.
    pub role: &'static str,
    /// A runner holds it now.
    pub live: bool,
    /// The live runner's session-map key (`None` when asleep).
    pub agent_name: Option<String>,
    /// The workspace that holds it, when known.
    pub project_id: Option<String>,
    /// The tab row's pane group (asleep or live tab).
    pub pane_group_id: Option<String>,
    /// `terminal` until C1's streamed runners exist.
    pub transport: &'static str,
    /// The conversation is the workspace's pinned chat (its row says so).
    pub canonical_row: bool,
}

fn role_for_key(key: &str, project_id: Option<&str>) -> &'static str {
    if project_id.is_some_and(|p| !p.is_empty() && key == p) {
        "main"
    } else if key.contains(":hb:") {
        "heartbeat"
    } else if k2_core::workspace_session_handles::is_api_agent_name(key) {
        "api"
    } else if k2_core::workspace_session_handles::is_tab_agent_name(key) {
        "tab"
    } else {
        "other"
    }
}

/// The live runner (not `exclude_agent`) whose argv resumes `id`.
fn live_runner(id: &str, exclude_agent: Option<&str>) -> Option<(String, Option<String>)> {
    v2_session_map::snapshot().into_iter().find_map(|(name, session)| {
        if exclude_agent == Some(name.as_str()) || !session.is_child_alive() {
            return None;
        }
        let live_id = crate::v2_spawn::conversation_id_from_args(session.program.as_deref(), &session.args)?;
        if live_id != id {
            return None;
        }
        let project = session.cwd.as_ref().and_then(|c| {
            let db = k2_core::db::shared();
            let conn = db.lock();
            k2_core::workspace::agent_identity::resolve_project_id(&conn, &c.to_string_lossy())
        });
        Some((name, project))
    })
}

/// Who holds conversation `id`. `project_id` scopes the pinned-chat check
/// to that workspace (`None`: any workspace's pinned chat). A runner
/// registered as `exclude_agent` (the door's own key) never counts.
pub fn conversation_owner(id: &str, project_id: Option<&str>, exclude_agent: Option<&str>) -> Option<Owner> {
    let id = id.trim();
    if id.is_empty() {
        return None;
    }
    let (main_project, tab_row) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let main_project = match project_id {
            Some(pid) => k2_core::db::schema::WorkspaceSession::get(&conn, pid)
                .ok()
                .flatten()
                .filter(|row| row.session_id.as_deref().map(str::trim) == Some(id))
                .map(|row| row.project_id),
            None => k2_core::db::schema::WorkspaceSession::get_by_session_id(&conn, id)
                .ok()
                .flatten()
                .map(|row| row.project_id),
        };
        let tab_row = k2_core::db::schema::WorkspaceTabSession::get_by_session_id(&conn, id)
            .ok()
            .flatten()
            .filter(|r| project_id.map_or(true, |p| r.project_id == p));
        (main_project, tab_row)
    };
    let live = live_runner(id, exclude_agent);

    if let Some(pid) = main_project {
        let canonical_live = v2_session_map::lookup_by_agent_name(&pid)
            .filter(|s| s.is_child_alive())
            .is_some()
            && exclude_agent != Some(pid.as_str());
        let (agent_name, is_live) = match &live {
            Some((name, _)) => (Some(name.clone()), true),
            None if canonical_live => (Some(pid.clone()), true),
            None => (None, false),
        };
        return Some(Owner {
            role: "main",
            live: is_live,
            agent_name,
            project_id: Some(pid),
            pane_group_id: None,
            transport: "terminal",
            canonical_row: true,
        });
    }
    if let Some((name, project)) = live {
        let project = project.or_else(|| project_id.map(str::to_string));
        let pane = name.strip_prefix("tab-").map(str::to_string);
        return Some(Owner {
            role: role_for_key(&name, project.as_deref()),
            live: true,
            agent_name: Some(name),
            project_id: project,
            pane_group_id: pane,
            transport: "terminal",
            canonical_row: false,
        });
    }
    tab_row.map(|row| Owner {
        role: "tab",
        live: false,
        agent_name: None,
        project_id: Some(row.project_id),
        pane_group_id: Some(row.pane_group_id),
        transport: "terminal",
        canonical_row: false,
    })
}

/// Why a spawn may not resume a conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplicateRefusal {
    pub conversation_id: String,
    /// The workspace's pinned chat owns it (its row, or it runs as main).
    pub canonical_owns: bool,
    /// The live runner's key, when one holds it.
    pub live_owner: Option<String>,
}

/// The duplicate check every non-canonical door runs before a spawn that
/// resumes `conversation_id` as `agent_name`: refused when the
/// workspace's pinned chat owns it, or another live runner resumes it.
/// An asleep tab row never refuses (resuming it is how it wakes).
pub fn duplicate_refusal(
    agent_name: &str,
    project_id: Option<&str>,
    conversation_id: Option<&str>,
) -> Option<DuplicateRefusal> {
    let id = conversation_id.map(str::trim).filter(|s| !s.is_empty())?;
    let owner = conversation_owner(id, project_id, Some(agent_name))?;
    let canonical_row = owner.canonical_row && project_id.is_some_and(|p| owner.project_id.as_deref() == Some(p));
    if !canonical_row && !owner.live {
        return None;
    }
    // A pinned chat of ANOTHER workspace that is asleep does not own this
    // workspace's tab (the scoped check above only reads this project's row).
    let canonical_owns = canonical_row
        || matches!((owner.agent_name.as_deref(), project_id), (Some(o), Some(p)) if o == p);
    log_debug!(
        "[conversation-owner] refusing duplicate of {id}: agent={agent_name} canonical={canonical_owns} live_owner={:?}",
        owner.agent_name
    );
    Some(DuplicateRefusal {
        conversation_id: id.to_string(),
        canonical_owns,
        live_owner: if owner.live { owner.agent_name } else { None },
    })
}

/// The error-string prefix an in-process spawn refusal carries, so a
/// door maps it to its 409 (`conversation_live`) and never a 500.
pub const SPAWN_REFUSAL_PREFIX: &str = "conversation_live: ";

/// [`duplicate_refusal`] as the spawn helper's error string.
pub fn spawn_refusal_text(r: &DuplicateRefusal) -> String {
    let who = if r.canonical_owns {
        "the workspace's main chat".to_string()
    } else {
        match &r.live_owner {
            Some(k) => format!("another live session ({k})"),
            None => "another live session".to_string(),
        }
    };
    format!(
        "{SPAWN_REFUSAL_PREFIX}conversation {} is already held by {who}; one conversation never runs twice (k2 sessions shows its address)",
        r.conversation_id
    )
}

/// The refusal sentence inside a spawn error, when it is one.
pub fn spawn_refusal_in(err: &str) -> Option<&str> {
    err.find(SPAWN_REFUSAL_PREFIX).map(|i| &err[i + SPAWN_REFUSAL_PREFIX.len()..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refusal_text_round_trips() {
        let r = DuplicateRefusal {
            conversation_id: "abc".into(),
            canonical_owns: false,
            live_owner: Some("tab-x".into()),
        };
        let text = spawn_refusal_text(&r);
        let wrapped = format!("spawn: {text}");
        let inner = spawn_refusal_in(&wrapped).expect("prefix found");
        assert!(inner.starts_with("conversation abc is already held by another live session (tab-x)"), "{inner}");
        assert_eq!(spawn_refusal_in("spawn_failed: other"), None);
    }

    #[test]
    fn roles_from_keys() {
        assert_eq!(role_for_key("p1", Some("p1")), "main");
        assert_eq!(role_for_key("p1:hb:nightly", Some("p1")), "heartbeat");
        assert_eq!(role_for_key("api-x-1", Some("p1")), "api");
        assert_eq!(role_for_key("tab-abc", Some("p1")), "tab");
        assert_eq!(role_for_key("other", None), "other");
    }
}
