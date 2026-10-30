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
//! **The one switch.** [`roster_json`] builds that shape from the sidecar
//! rows until `crate::workspace_roster` is on this branch; then its body
//! becomes
//! `crate::workspace_roster::workspace_roster(project_id, &RosterOpts { all: true, include_api: false, caller_map_key: None }).map(|r| r.to_json())`
//! and nothing else in the Workbench changes.

use serde_json::{json, Value as J};

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
pub fn sidecars(project_id: &str, ws_path: &str) -> Result<Vec<SessionRow>, String> {
    Ok(rows_from_roster(&roster_json(project_id, ws_path)?))
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

/// The workspace's sessions in the shared list's shape (contract §2), the
/// sidecar part only, until `workspace_roster` is on this branch.
fn roster_json(project_id: &str, ws_path: &str) -> Result<J, String> {
    let rows = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::sidecar::list_rows(&conn, project_id)?
    };
    let sessions: Vec<J> = rows
        .into_iter()
        .map(|r| {
            let live = crate::v2_session_map::lookup_by_agent_name(&r.agent_name).is_some_and(|x| x.is_child_alive());
            let worktree = r.cwd.as_deref().and_then(|c| worktree_of(c, ws_path));
            json!({
                "handle": r.handle,
                "role": if worktree.is_some() { "worktree" } else { "sidecar" },
                "cli": r.provider,
                "title": r.name,
                "agentName": r.agent_name,
                "paneGroupId": r.pane_group_id,
                "worktree": worktree,
                "branch": J::Null,
                "state": if live { "idle" } else { "asleep" },
            })
        })
        .collect();
    Ok(json!({ "ok": true, "sessions": sessions }))
}

/// `cwd` relative to the workspace when it is a folder inside it.
fn worktree_of(cwd: &str, ws_path: &str) -> Option<String> {
    let c = dunce::canonicalize(cwd).ok()?;
    let p = dunce::canonicalize(ws_path).unwrap_or_else(|_| ws_path.into());
    let rel = c.strip_prefix(&p).ok()?.to_string_lossy().replace('\\', "/");
    (!rel.is_empty() && c.is_dir()).then_some(rel)
}

#[cfg(test)]
mod tests {
    use super::*;

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
