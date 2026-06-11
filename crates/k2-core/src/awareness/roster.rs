//! Roster — "who's in my team?"
//!
//! 0.39.0 rewrite (workspace==agent model). Pre-0.39 the roster
//! enumerated `.k2so/agents/<name>/` subdirectories under a workspace
//! root and called those entries "agents." That mental model belonged
//! to the pre-Phase-2.1 multi-agent-per-workspace world; post-0.39 a
//! workspace IS an agent, and the team for that agent is the set of
//! OTHER CONNECTED WORKSPACES.
//!
//! New data sources:
//!
//! - **Known peers** → [`crate::connections::list_peers`]. Bidirectional,
//!   deduped — whoever initiated the relationship, both sides see the
//!   other as just "connected." The roster never surfaces direction.
//! - **Liveness** → [`crate::session::registry`]. For each peer name,
//!   check whether any registered session has that agent_name binding.
//!   A peer with no live session is `Offline`.
//! - **Skill summary** → `None` for now. Pre-0.39 this was read from
//!   `.k2so/agents/<name>/SKILL.md` inside the calling workspace's
//!   tree; peers live in SEPARATE filesystem trees, so reading their
//!   SKILL.md cleanly would require either an MCP-style read endpoint
//!   on each peer's daemon, or path enumeration via the `projects`
//!   table — neither is in scope for this commit. Follow-up:
//!   `.k2so/inbox/issues/peer-skill-summary-read.md` once the daemon
//!   exposes a `/cli/skills/profile` over a connected workspace.
//!
//! All filter shapes still exist (`LiveInWorkspace`, `LiveEverywhere`,
//! `AllKnown`) so existing call sites compile without churn, but
//! their semantics shift to peer-centric:
//!
//! - `AllKnown(path)` — every peer connected to the workspace at `path`
//! - `LiveInWorkspace(path)` — peers connected to `path` that are
//!   currently live (have a registered session)
//! - `LiveEverywhere` — every live session anywhere on this daemon,
//!   regardless of which workspaces are wired up to each other
//!
//! No provider trait here (unlike `companion::settings_bridge`) —
//! the data sources are already singletons (`crate::db::shared`,
//! `crate::session::registry`).

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::awareness::WorkspaceId;
use crate::session::registry;

/// Filter for `query`. Three shapes covering the common questions:
/// "who can I talk to here?", "who's live anywhere?", "who exists
/// at all?"
#[derive(Debug, Clone)]
pub enum RosterFilter<'a> {
    /// Live peers connected to a specific workspace root.
    LiveInWorkspace(&'a Path),
    /// Live agents anywhere the session::registry knows about.
    /// Unlike the workspace-scoped filters, this one ignores
    /// `workspace_relations` and just reports whoever has a live
    /// session right now — useful for daemon-wide diagnostics.
    LiveEverywhere,
    /// All known peers of the workspace at this root, live or offline.
    AllKnown(&'a Path),
}

/// Per-peer roster entry. Carries enough for a caller to render
/// a list, choose a target, and decide whether to interrupt.
///
/// 0.39.0 note: `skill_summary` is now `Option<String>` and currently
/// always `None` for peers — see the module head for why. The shape
/// stays similar to the pre-0.39 `AgentInfo` so existing callers
/// keep compiling; only the semantics of `name` shifts ("peer
/// workspace name" instead of "agent subdirectory name").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentInfo {
    /// Peer workspace's display name (matches `projects.name`).
    pub name: String,
    /// Workspace this entry is "about" — set for the workspace-scoped
    /// filters, `None` for `LiveEverywhere`.
    pub workspace: Option<WorkspaceId>,
    pub state: RosterState,
    /// Documentation summary for the peer. `None` for now —
    /// cross-workspace skill reads are out of scope for the 0.39.0
    /// roster rewrite; see the module head for context.
    #[serde(default)]
    pub skill_summary: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
#[serde(rename_all = "lowercase")]
pub enum RosterState {
    Live,
    Offline,
}

/// Run a roster query. Never errors — DB / registry read failures
/// return empty / skip-entry semantics rather than propagating.
/// The roster is a best-effort view; callers should treat its
/// output as "the peers I currently know about" not "all peers
/// that will ever exist."
pub fn query(filter: RosterFilter<'_>) -> Vec<AgentInfo> {
    match filter {
        RosterFilter::LiveEverywhere => live_agents_anywhere(),
        RosterFilter::LiveInWorkspace(root) => {
            let mut out = scan_workspace_peers(root);
            out.retain(|info| info.state == RosterState::Live);
            out
        }
        RosterFilter::AllKnown(root) => scan_workspace_peers(root),
    }
}

/// Convenience: look up one specific peer of the given workspace.
/// Returns `None` if no `workspace_relations` row connects the
/// caller's workspace to a peer with that name.
pub fn lookup(workspace_root: &Path, peer_name: &str) -> Option<AgentInfo> {
    scan_workspace_peers(workspace_root)
        .into_iter()
        .find(|info| info.name == peer_name)
}

// ─────────────────────────────────────────────────────────────────────
// Internals
// ─────────────────────────────────────────────────────────────────────

fn live_agents_anywhere() -> Vec<AgentInfo> {
    let live = live_agent_names();
    live.into_iter()
        .map(|name| AgentInfo {
            name,
            workspace: None,
            state: RosterState::Live,
            skill_summary: None,
        })
        .collect()
}

/// Build the peer list for a workspace by calling
/// [`crate::connections::list_peers`] and overlaying liveness from
/// the session registry.
///
/// Returns an empty Vec if the workspace isn't registered (the
/// `list_peers` call returns `Err("Project not found …")`), if the
/// workspace has no connections, or if the registry has no live
/// sessions to overlay. Output is sorted alphabetically by peer
/// name (inherited from `list_peers`).
fn scan_workspace_peers(workspace_root: &Path) -> Vec<AgentInfo> {
    let path_str = workspace_root.to_string_lossy().into_owned();
    let peers = match crate::connections::list_peers(&path_str) {
        Ok(p) => p,
        // Unregistered workspace / DB miss → empty roster, never panic.
        // The roster is a "what do I currently see" surface; absence
        // of state is a perfectly valid answer.
        Err(_) => return Vec::new(),
    };
    let live = live_agent_names();
    let ws_id = WorkspaceId(path_str);

    peers
        .into_iter()
        .map(|peer| {
            let state = if live.iter().any(|n| n == &peer.project_name) {
                RosterState::Live
            } else {
                RosterState::Offline
            };
            AgentInfo {
                name: peer.project_name,
                workspace: Some(ws_id.clone()),
                state,
                skill_summary: None,
            }
        })
        .collect()
}

/// Enumerate agent names that currently have a live session in
/// `session::registry`. A session without an `agent_name` binding
/// (anonymous / test fixtures) is skipped.
fn live_agent_names() -> Vec<String> {
    registry::list_ids()
        .into_iter()
        .filter_map(|id| registry::lookup(&id).and_then(|e| e.agent_name()))
        .collect()
}

#[cfg(test)]
mod tests {
    //! Tests for the 0.39.0 roster rewrite — verifies the data source
    //! is connections, not `.k2so/agents/` directory walk.
    //!
    //! Each test seeds its own UUID-keyed `projects` rows so it
    //! doesn't collide with other tests sharing the process-wide
    //! in-memory DB.
    use super::*;
    use crate::db::schema::WorkspaceRelation;
    use std::path::PathBuf;
    use uuid::Uuid;

    fn make_project(name: &str) -> (PathBuf, String) {
        let db = crate::db::shared();
        let conn = db.lock();
        let path = std::env::temp_dir().join(format!(
            "k2so-roster-{}-{}",
            name,
            Uuid::new_v4()
        ));
        std::fs::create_dir_all(&path).expect("scratch dir");
        let id = Uuid::new_v4().to_string();
        // Pin a deterministic name we can find later via the roster.
        let project_name = format!("{}-{}", name, &id[..8]);
        conn.execute(
            "INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
            rusqlite::params![id, project_name, path.to_string_lossy()],
        )
        .expect("insert project");
        (path, id)
    }

    #[test]
    fn roster_uses_connections_as_known_peers_source() {
        // Seed an `me` workspace + two peer workspaces; wire up
        // one outgoing connection (me → peer_a) and one incoming
        // (peer_b → me). Both peers should appear in `AllKnown`.
        let (me_root, me_id) = make_project("uses-conn-me");
        let (_a_root, a_id) = make_project("uses-conn-peer-a");
        let (_b_root, b_id) = make_project("uses-conn-peer-b");
        {
            let db = crate::db::shared();
            let conn = db.lock();
            // outgoing: me → a
            WorkspaceRelation::create(
                &conn,
                &Uuid::new_v4().to_string(),
                &me_id,
                &a_id,
                "oversees",
            )
            .unwrap();
            // incoming: b → me
            WorkspaceRelation::create(
                &conn,
                &Uuid::new_v4().to_string(),
                &b_id,
                &me_id,
                "collaborator",
            )
            .unwrap();
        }

        let out = query(RosterFilter::AllKnown(&me_root));
        assert_eq!(
            out.len(),
            2,
            "both peers (outgoing AND incoming) should appear; got {out:?}"
        );

        let names: Vec<&str> = out.iter().map(|i| i.name.as_str()).collect();
        // list_peers sorts alphabetically by name. We don't know the
        // exact suffixes, so just assert both peer-name prefixes are
        // present.
        assert!(
            names.iter().any(|n| n.starts_with("uses-conn-peer-a-")),
            "peer A should be present; names={names:?}"
        );
        assert!(
            names.iter().any(|n| n.starts_with("uses-conn-peer-b-")),
            "peer B should be present; names={names:?}"
        );
        // skill_summary is None for every peer until cross-workspace
        // skill reads land — see module head.
        for entry in &out {
            assert!(
                entry.skill_summary.is_none(),
                "skill_summary should be None pending cross-workspace read; got {entry:?}"
            );
        }
        std::fs::remove_dir_all(&me_root).ok();
    }

    #[test]
    fn roster_empty_when_no_connections() {
        let (me_root, _me_id) = make_project("empty-roster");
        // No WorkspaceRelation::create calls — workspace exists in
        // `projects` but has zero connections.
        let out = query(RosterFilter::AllKnown(&me_root));
        assert!(
            out.is_empty(),
            "unconnected workspace must produce empty roster; got {out:?}"
        );
        std::fs::remove_dir_all(&me_root).ok();
    }

    #[test]
    fn roster_empty_when_workspace_unregistered() {
        // No `projects` row for this path at all → roster is empty
        // (never panics, never errors).
        let bogus_root = std::env::temp_dir().join(format!(
            "k2so-roster-unregistered-{}",
            Uuid::new_v4()
        ));
        std::fs::create_dir_all(&bogus_root).unwrap();
        let out = query(RosterFilter::AllKnown(&bogus_root));
        assert!(
            out.is_empty(),
            "unregistered workspace path must produce empty roster; got {out:?}"
        );
        std::fs::remove_dir_all(&bogus_root).ok();
    }

    #[test]
    fn roster_marks_peers_with_live_sessions_as_online() {
        // Seed a peer connected to `me`, then register a session
        // whose agent_name binding matches the peer's project_name.
        // The roster should mark that peer Live.
        let (me_root, me_id) = make_project("liveness-me");
        let (_peer_root, peer_id) = make_project("liveness-peer");
        let peer_name: String = {
            let db = crate::db::shared();
            let conn = db.lock();
            WorkspaceRelation::create(
                &conn,
                &Uuid::new_v4().to_string(),
                &me_id,
                &peer_id,
                "oversees",
            )
            .unwrap();
            conn.query_row(
                "SELECT name FROM projects WHERE id = ?1",
                rusqlite::params![peer_id],
                |row| row.get(0),
            )
            .unwrap()
        };

        // Register a session bound to the peer's project_name.
        let sess_id = crate::session::SessionId::new();
        let entry = registry::register(sess_id);
        entry.set_agent_name(&peer_name);

        let out = query(RosterFilter::AllKnown(&me_root));
        let peer_entry = out
            .iter()
            .find(|i| i.name == peer_name)
            .unwrap_or_else(|| panic!("peer '{peer_name}' missing from roster: {out:?}"));
        assert_eq!(
            peer_entry.state,
            RosterState::Live,
            "peer with registered session must be Live"
        );

        // LiveInWorkspace must include it.
        let live_only = query(RosterFilter::LiveInWorkspace(&me_root));
        assert!(
            live_only.iter().any(|i| i.name == peer_name),
            "LiveInWorkspace should include the live peer; got {live_only:?}"
        );

        // Cleanup the registry binding so other tests aren't polluted.
        registry::unregister(&sess_id);
        std::fs::remove_dir_all(&me_root).ok();
    }

    #[test]
    fn agent_info_json_serializes_with_skill_summary_optional() {
        // Regression: AgentInfo's skill_summary is now Option<String>;
        // None must serialize as either absent (via skip_serializing_if)
        // or as `null` — and round-trip cleanly either way.
        let info = AgentInfo {
            name: "alice".into(),
            workspace: Some(WorkspaceId("k2so".into())),
            state: RosterState::Live,
            skill_summary: None,
        };
        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains(r#""state":"live""#), "{json}");
        let decoded: AgentInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(info, decoded);

        // And with a populated summary it round-trips too.
        let with_summary = AgentInfo {
            name: "bob".into(),
            workspace: None,
            state: RosterState::Offline,
            skill_summary: Some("does things".into()),
        };
        let json = serde_json::to_string(&with_summary).unwrap();
        let decoded: AgentInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(with_summary, decoded);
    }
}
