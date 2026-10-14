//! Garden data grants: the `zen_data_grants` table in k2so.db (migration
//! 0164; contract §2.1, §3.4; PRD §5.5). C0 → B1.
//!
//! - Default: no agent access. A grant names one agent (a workspace) or a
//!   K2 project (every member, resolved live, **convenience-level only**:
//!   any agent may join a project, Rosson 2026-10-14).
//! - A delegation (`admin = 1`) lets one agent grant inside its prefix, up
//!   to its level, never to itself, never re-delegating, revoking only what
//!   it made (PRD §5.5.3). Revoking a delegation cascades.
//! - Checked on every agent call; no cache.

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use super::{keys, not_built, DataError, Level, Space};

/// What a grant names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SubjectKind {
    /// One agent: `projects.id`.
    Workspace,
    /// Every member of a K2 project: `project_groups.id`.
    Project,
}

impl SubjectKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SubjectKind::Workspace => "workspace",
            SubjectKind::Project => "project",
        }
    }
}

/// One `zen_data_grants` row.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GrantRow {
    pub id: i64,
    pub space: String,
    pub prefix: String,
    pub subject_kind: SubjectKind,
    pub subject_id: String,
    pub level: Level,
    pub admin: bool,
    pub via_grant: Option<i64>,
    pub granted_by: String,
    pub granted_at: i64,
}

/// A grant to write (person or delegate).
#[derive(Debug, Clone, PartialEq)]
pub struct NewGrant {
    pub space: Space,
    pub prefix: String,
    pub subject_kind: SubjectKind,
    pub subject_id: String,
    pub level: Level,
    pub admin: bool,
    pub via_grant: Option<i64>,
    /// `owner` | `agent:<projects.id>`.
    pub granted_by: String,
}

/// Where an effective grant comes from (`k2 store grants`, BG9).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum GrantFrom {
    Agent,
    Project { id: String, name: String },
    Delegation,
}

/// One grant that applies to a caller now.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EffectiveGrant {
    pub prefix: String,
    pub level: Level,
    pub from: GrantFrom,
    pub admin: bool,
    pub grant_id: i64,
}

/// A caller's access to one space, resolved once per request.
#[derive(Debug, Clone, PartialEq)]
pub enum Access {
    /// The owner, or an owner/K2-made widget.
    Full,
    /// An agent, or an agent-made widget (its maker's grants).
    Grants(Vec<EffectiveGrant>),
    /// Nothing.
    None,
}

impl Access {
    fn allows(&self, key: &str, need: Level) -> bool {
        match self {
            Access::Full => true,
            Access::None => false,
            Access::Grants(gs) => gs.iter().any(|g| g.level.covers(need) && keys::under(&g.prefix, key)),
        }
    }

    pub fn can_read(&self, key: &str) -> bool {
        self.allows(key, Level::Read)
    }

    pub fn can_write(&self, key: &str) -> bool {
        self.allows(key, Level::Write)
    }

    /// At least one write grant anywhere in the space (`tx/begin`).
    pub fn can_write_somewhere(&self) -> bool {
        match self {
            Access::Full => true,
            Access::None => false,
            Access::Grants(gs) => gs.iter().any(|g| g.level == Level::Write),
        }
    }

    /// Read prefixes for list/changes/export filtering. `None` = no filter.
    pub fn read_prefixes(&self) -> Option<Vec<String>> {
        match self {
            Access::Full => None,
            Access::None => Some(Vec::new()),
            Access::Grants(gs) => Some(gs.iter().map(|g| g.prefix.clone()).collect()),
        }
    }
}

/// Direct grants + the grants of every K2 project the workspace is a member
/// of right now (`project_group_members`, idx 0066) + its delegations.
pub fn effective_grants(_conn: &Connection, _space: &Space, _project_id: &str) -> Result<Vec<EffectiveGrant>, DataError> {
    Err(not_built())
}

/// `Access::Grants(effective_grants(..))`, or `Access::None` when empty.
pub fn access_for_agent(_conn: &Connection, _space: &Space, _project_id: &str) -> Result<Access, DataError> {
    Err(not_built())
}

/// Every row of a space (the person's listing).
pub fn list_space(_conn: &Connection, _space: &Space) -> Result<Vec<GrantRow>, DataError> {
    Err(not_built())
}

/// Insert or update by `(space, prefix, subject)`. Answers the row and
/// whether it was created.
pub fn upsert(_conn: &Connection, _g: &NewGrant, _now: i64) -> Result<(GrantRow, bool), DataError> {
    Err(not_built())
}

/// Delete one row; a delegation takes every `via_grant` row with it.
/// Answers how many delegate-made rows went with it.
pub fn remove(_conn: &Connection, _id: i64) -> Result<usize, DataError> {
    Err(not_built())
}

/// The delegations a workspace holds in a space.
pub fn delegations_of(_conn: &Connection, _space: &Space, _project_id: &str) -> Result<Vec<GrantRow>, DataError> {
    Err(not_built())
}

/// Pure: may this delegate make this grant? (prefix inside its own, level
/// at most its own, not itself, never a delegation). PRD §5.5.3.
pub fn check_delegate_grant(_delegation: &GrantRow, _req: &NewGrant, _caller_project: &str) -> Result<(), DataError> {
    Err(not_built())
}

/// Workspace delete (same transaction as `projects_delete`): its rows,
/// plus every row made under its delegations. Answers rows removed.
pub fn delete_for_workspace(_conn: &Connection, _project_id: &str) -> Result<usize, DataError> {
    Err(not_built())
}

/// Project-group delete: the rows that name it.
pub fn delete_for_project_group(_conn: &Connection, _group_id: &str) -> Result<usize, DataError> {
    Err(not_built())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g(prefix: &str, level: Level) -> EffectiveGrant {
        EffectiveGrant { prefix: prefix.into(), level, from: GrantFrom::Agent, admin: false, grant_id: 1 }
    }

    #[test]
    fn access_checks_prefix_and_level() {
        let a = Access::Grants(vec![g("project/7/", Level::Write), g("shared/", Level::Read)]);
        assert!(a.can_write("project/7/board/1/obj/9"));
        assert!(a.can_read("project/7/x"), "write covers read");
        assert!(a.can_read("shared/x") && !a.can_write("shared/x"));
        assert!(!a.can_read("project/70/x"));
        assert!(a.can_write_somewhere());
        assert_eq!(a.read_prefixes(), Some(vec!["project/7/".to_string(), "shared/".to_string()]));
        assert!(Access::Full.can_write("anything") && Access::Full.read_prefixes().is_none());
        assert!(!Access::None.can_read("x") && Access::None.read_prefixes() == Some(vec![]));
    }
}
