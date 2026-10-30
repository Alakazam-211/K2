//! The Workbench's one read of "which agent sessions does a workspace have"
//! (0.46.7). Every Workbench list (`work.list`, `GET /cli/workbench/agents`,
//! an address's sidecar) goes through this file and nothing else.
//!
//! It reads the shared workspace session list in its contract shape
//! (`.k2/prds/workspace-sessions-contract.md` §2, lane
//! `k2/workspace-sessions`: one builder, one list) and keeps only the rows
//! the Workbench shows: roles `sidecar` and `worktree` with a handle.
//! Unknown roles are ignored (contract §3).
//!
//! **The one switch.** [`roster_json`] is the shared list itself:
//! `crate::workspace_roster::workspace_roster` with every row (asleep ones
//! too), no API sessions and no caller. Worktree sidecars come back as role
//! `worktree` with their folder and branch (W1's columns, migration 0251).

use serde_json::Value as J;

/// A workspace on this computer.
pub struct Workspace {
    pub id: String,
    pub name: String,
    pub path: String,
}

/// One Workbench task: a sidecar row of the shared list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRow {
    /// `reviewer`, `fix-login` (the address is `<ws>/<handle>`).
    pub handle: String,
    pub role: String,
    pub title: Option<String>,
    /// The provider (claude, codex, …).
    pub cli: Option<String>,
    /// The `v2_session_map` key (the Workbench joins on it).
    pub agent_name: Option<String>,
    pub pane_group_id: Option<String>,
    /// Its worktree relative to the workspace (`.worktrees/k2-fix`).
    pub worktree: Option<String>,
    pub branch: Option<String>,
    /// working | waiting | idle | asleep.
    pub state: String,
}

impl SessionRow {
    pub fn live(&self) -> bool {
        self.state != "asleep"
    }
}

/// Every workspace on this computer.
pub fn workspaces() -> Result<Vec<Workspace>, String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    Ok(k2_core::db::schema::Project::list(&conn)
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|p| Workspace { id: p.id, name: p.name, path: p.path })
        .collect())
}

/// A workspace's sidecars (its main agent is not one), from the shared list.
pub fn sidecars(project_id: &str) -> Result<Vec<SessionRow>, String> {
    Ok(rows_from_roster(&roster_json(project_id)?))
}

/// The shared list's sidecar rows (contract §2): `sidecar` / `worktree`
/// roles with a handle; everything else is left out.
pub fn rows_from_roster(v: &J) -> Vec<SessionRow> {
    let s = |x: &J, k: &str| x.get(k).and_then(J::as_str).map(str::trim).filter(|t| !t.is_empty()).map(str::to_string);
    v.get("sessions")
        .and_then(J::as_array)
        .into_iter()
        .flatten()
        .filter_map(|r| {
            let role = s(r, "role")?;
            if role != "sidecar" && role != "worktree" {
                return None;
            }
            Some(SessionRow {
                handle: s(r, "handle")?,
                role,
                title: s(r, "title"),
                cli: s(r, "cli"),
                agent_name: s(r, "agentName"),
                pane_group_id: s(r, "paneGroupId"),
                worktree: s(r, "worktree"),
                branch: s(r, "branch"),
                state: s(r, "state").unwrap_or_else(|| "asleep".to_string()),
            })
        })
        .collect()
}

/// The workspace's sessions in the shared list's shape (contract §2).
fn roster_json(project_id: &str) -> Result<J, String> {
    let opts = crate::workspace_roster::RosterOpts { all: true, include_api: false, caller_map_key: None };
    crate::workspace_roster::workspace_roster(project_id, &opts).map(|r| r.to_json())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The contract's example rows (§2): only sidecar and worktree rows with
    /// a handle are Workbench tasks; unknown roles are ignored.
    #[test]
    fn reads_the_shared_list_shape() {
        let v = json!({ "ok": true, "sessions": [
            { "address": "k2", "handle": null, "role": "main", "cli": "claude", "state": "working" },
            { "address": "k2/fix", "handle": "fix", "role": "worktree", "cli": "claude", "state": "working", "title": "Fix login",
              "agentName": "tab-pane-1", "paneGroupId": "pane-1", "worktree": ".worktrees/k2-fix", "branch": "k2/fix" },
            { "address": "k2/rev", "handle": "rev", "role": "sidecar", "cli": "codex", "state": "asleep" },
            { "address": "k2/x", "handle": "x", "role": "streamed-thing", "state": "idle" },
            { "address": null, "handle": "hb:nightly", "role": "heartbeat", "state": "idle" },
            { "address": "k2/nohandle", "handle": null, "role": "sidecar", "state": "idle" }
        ]});
        let rows = rows_from_roster(&v);
        assert_eq!(rows.len(), 2, "{rows:?}");
        assert_eq!(rows[0].handle, "fix");
        assert_eq!(rows[0].worktree.as_deref(), Some(".worktrees/k2-fix"));
        assert_eq!(rows[0].branch.as_deref(), Some("k2/fix"));
        assert!(rows[0].live());
        assert_eq!((rows[1].handle.as_str(), rows[1].cli.as_deref(), rows[1].live()), ("rev", Some("codex"), false));
        assert!(rows_from_roster(&json!({ "ok": true })).is_empty());
    }
}
