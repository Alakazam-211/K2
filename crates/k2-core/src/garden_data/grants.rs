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

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use super::{keys, DataError, Level, Space};

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

const COLS: &str =
    "g.id, g.space, g.prefix, g.subject_kind, g.subject_id, g.level, g.admin, g.via_grant, g.granted_by, g.granted_at";

fn row(r: &rusqlite::Row) -> rusqlite::Result<GrantRow> {
    let kind: String = r.get(3)?;
    let level: String = r.get(5)?;
    Ok(GrantRow {
        id: r.get(0)?,
        space: r.get(1)?,
        prefix: r.get(2)?,
        subject_kind: if kind == "project" { SubjectKind::Project } else { SubjectKind::Workspace },
        subject_id: r.get(4)?,
        level: Level::parse(&level).unwrap_or(Level::Read),
        admin: r.get::<_, i64>(6)? == 1,
        via_grant: r.get(7)?,
        granted_by: r.get(8)?,
        granted_at: r.get(9)?,
    })
}

fn data_space(space: &Space) -> Result<(), DataError> {
    if space.is_data() {
        Ok(())
    } else {
        Err(DataError::BadRequest("saves are private to their widget and are never granted".into()))
    }
}

/// Direct grants + the grants of every K2 project the workspace is a member
/// of right now (`project_group_members`, idx 0066) + its delegations.
/// Ordered by prefix, then id.
pub fn effective_grants(conn: &Connection, space: &Space, project_id: &str) -> Result<Vec<EffectiveGrant>, DataError> {
    let mut out = Vec::new();
    {
        let mut q = conn.prepare_cached(&format!(
            "SELECT {COLS} FROM zen_data_grants g
             WHERE g.space = ?1 AND g.subject_kind = 'workspace' AND g.subject_id = ?2"
        ))?;
        for g in q.query_map(params![space.as_str(), project_id], row)? {
            let g = g?;
            out.push(EffectiveGrant {
                prefix: g.prefix,
                level: g.level,
                from: if g.admin { GrantFrom::Delegation } else { GrantFrom::Agent },
                admin: g.admin,
                grant_id: g.id,
            });
        }
    }
    {
        let mut q = conn.prepare_cached(&format!(
            "SELECT {COLS}, pg.name FROM zen_data_grants g
             JOIN project_group_members m ON m.group_id = g.subject_id
             JOIN project_groups pg ON pg.id = g.subject_id
             WHERE g.space = ?1 AND g.subject_kind = 'project' AND m.workspace_id = ?2"
        ))?;
        let rows = q.query_map(params![space.as_str(), project_id], |r| Ok((row(r)?, r.get::<_, String>(10)?)))?;
        for x in rows {
            let (g, name) = x?;
            out.push(EffectiveGrant {
                prefix: g.prefix,
                level: g.level,
                from: GrantFrom::Project { id: g.subject_id, name },
                admin: false,
                grant_id: g.id,
            });
        }
    }
    out.sort_by(|a, b| a.prefix.cmp(&b.prefix).then(a.grant_id.cmp(&b.grant_id)));
    Ok(out)
}

/// `Access::Grants(effective_grants(..))`, or `Access::None` when empty.
pub fn access_for_agent(conn: &Connection, space: &Space, project_id: &str) -> Result<Access, DataError> {
    let gs = effective_grants(conn, space, project_id)?;
    Ok(if gs.is_empty() { Access::None } else { Access::Grants(gs) })
}

/// Every row of a space (the person's listing), by prefix then id.
pub fn list_space(conn: &Connection, space: &Space) -> Result<Vec<GrantRow>, DataError> {
    let mut q =
        conn.prepare_cached(&format!("SELECT {COLS} FROM zen_data_grants g WHERE g.space = ?1 ORDER BY g.prefix, g.id"))?;
    let rows = q.query_map([space.as_str()], row)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(DataError::from)
}

/// One row by id.
pub fn by_id(conn: &Connection, id: i64) -> Result<Option<GrantRow>, DataError> {
    conn.query_row(&format!("SELECT {COLS} FROM zen_data_grants g WHERE g.id = ?1"), [id], row)
        .optional()
        .map_err(DataError::from)
}

/// The row for `(space, prefix, subject)`, if any.
pub fn find(
    conn: &Connection,
    space: &Space,
    prefix: &str,
    kind: SubjectKind,
    subject_id: &str,
) -> Result<Option<GrantRow>, DataError> {
    conn.query_row(
        &format!(
            "SELECT {COLS} FROM zen_data_grants g
             WHERE g.space = ?1 AND g.prefix = ?2 AND g.subject_kind = ?3 AND g.subject_id = ?4"
        ),
        params![space.as_str(), prefix, kind.as_str(), subject_id],
        row,
    )
    .optional()
    .map_err(DataError::from)
}

/// Insert or update by `(space, prefix, subject)`. Answers the row and
/// whether it was created.
pub fn upsert(conn: &Connection, g: &NewGrant, now: i64) -> Result<(GrantRow, bool), DataError> {
    data_space(&g.space)?;
    keys::validate_prefix(&g.prefix)?;
    if g.subject_id.is_empty() {
        return Err(DataError::BadRequest("a grant names an agent or a project".into()));
    }
    let existed = find(conn, &g.space, &g.prefix, g.subject_kind, &g.subject_id)?.is_some();
    conn.execute(
        "INSERT INTO zen_data_grants (space, prefix, subject_kind, subject_id, level, admin, via_grant, granted_by, granted_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(space, prefix, subject_kind, subject_id) DO UPDATE SET level = excluded.level,
           admin = excluded.admin, via_grant = excluded.via_grant, granted_by = excluded.granted_by,
           granted_at = excluded.granted_at",
        params![
            g.space.as_str(),
            g.prefix,
            g.subject_kind.as_str(),
            g.subject_id,
            g.level.as_str(),
            g.admin as i64,
            g.via_grant,
            g.granted_by,
            now
        ],
    )?;
    let r = find(conn, &g.space, &g.prefix, g.subject_kind, &g.subject_id)?
        .ok_or_else(|| DataError::Unavailable("the grant vanished after it was written".into()))?;
    Ok((r, !existed))
}

/// Delete one row; a delegation takes every `via_grant` row with it (one
/// transaction). Answers how many delegate-made rows went with it.
pub fn remove(conn: &Connection, id: i64) -> Result<usize, DataError> {
    let tx = conn.unchecked_transaction()?;
    let cascaded = tx.execute("DELETE FROM zen_data_grants WHERE via_grant = ?1", [id])?;
    let n = tx.execute("DELETE FROM zen_data_grants WHERE id = ?1", [id])?;
    if n == 0 {
        return Err(DataError::NotFound("grant_not_found"));
    }
    tx.commit()?;
    Ok(cascaded)
}

/// How many rows were made under a delegation (the dry run before a revoke).
pub fn made_under(conn: &Connection, id: i64) -> Result<usize, DataError> {
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM zen_data_grants WHERE via_grant = ?1", [id], |r| r.get(0))?;
    Ok(n as usize)
}

/// The rows made under one delegation.
pub fn list_made_under(conn: &Connection, id: i64) -> Result<Vec<GrantRow>, DataError> {
    let mut q = conn
        .prepare_cached(&format!("SELECT {COLS} FROM zen_data_grants g WHERE g.via_grant = ?1 ORDER BY g.prefix, g.id"))?;
    let rows = q.query_map([id], row)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(DataError::from)
}

/// The delegations a workspace holds in a space (shortest prefix first).
pub fn delegations_of(conn: &Connection, space: &Space, project_id: &str) -> Result<Vec<GrantRow>, DataError> {
    let mut q = conn.prepare_cached(&format!(
        "SELECT {COLS} FROM zen_data_grants g
         WHERE g.space = ?1 AND g.subject_kind = 'workspace' AND g.subject_id = ?2 AND g.admin = 1
         ORDER BY length(g.prefix), g.id"
    ))?;
    let rows = q.query_map(params![space.as_str(), project_id], row)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(DataError::from)
}

/// Pure: may this delegate make this grant? Prefix inside its own, level at
/// most its own, never a delegation, never itself (PRD §5.5.3).
pub fn check_delegate_grant(delegation: &GrantRow, req: &NewGrant, caller_project: &str) -> Result<(), DataError> {
    if !delegation.admin || delegation.space != req.space.as_str() || !req.prefix.starts_with(&delegation.prefix) {
        return Err(DataError::NotGranted { key: None });
    }
    if req.admin {
        return Err(DataError::Refused {
            code: "not_granted",
            status: 403,
            hint: "a grant admin can't make another grant admin; only the person can".into(),
        });
    }
    if !delegation.level.covers(req.level) {
        return Err(DataError::Refused {
            code: "level_too_high",
            status: 403,
            hint: format!("your delegation is {0}; you can grant {0} only", delegation.level.as_str()),
        });
    }
    if req.subject_kind == SubjectKind::Workspace && req.subject_id == caller_project {
        return Err(DataError::Refused {
            code: "self_grant",
            status: 403,
            hint: "a grant admin never grants itself; ask the person".into(),
        });
    }
    Ok(())
}

/// Workspace delete: its rows, plus every row made under its delegations.
/// Answers rows removed.
pub fn delete_for_workspace(conn: &Connection, project_id: &str) -> Result<usize, DataError> {
    let made = conn.execute(
        "DELETE FROM zen_data_grants WHERE via_grant IN
           (SELECT id FROM zen_data_grants WHERE subject_kind = 'workspace' AND subject_id = ?1 AND admin = 1)",
        [project_id],
    )?;
    let own =
        conn.execute("DELETE FROM zen_data_grants WHERE subject_kind = 'workspace' AND subject_id = ?1", [project_id])?;
    Ok(made + own)
}

/// Project-group delete: the rows that name it.
pub fn delete_for_project_group(conn: &Connection, group_id: &str) -> Result<usize, DataError> {
    Ok(conn.execute("DELETE FROM zen_data_grants WHERE subject_kind = 'project' AND subject_id = ?1", [group_id])?)
}

/// Space delete: every row of the space.
pub fn delete_for_space(conn: &Connection, space: &Space) -> Result<usize, DataError> {
    Ok(conn.execute("DELETE FROM zen_data_grants WHERE space = ?1", [space.as_str()])?)
}

/// Space move: the rows follow the space (any rows already on `to` go).
pub fn move_space(conn: &Connection, from: &Space, to: &Space) -> Result<usize, DataError> {
    data_space(to)?;
    let tx = conn.unchecked_transaction()?;
    tx.execute("DELETE FROM zen_data_grants WHERE space = ?1", [to.as_str()])?;
    let n = tx.execute("UPDATE zen_data_grants SET space = ?2 WHERE space = ?1", params![from.as_str(), to.as_str()])?;
    tx.commit()?;
    Ok(n)
}

/// Per space: grant count and admin workspace ids (Settings' list).
pub fn summary(conn: &Connection) -> Result<Vec<(String, usize, Vec<String>)>, DataError> {
    let mut q = conn.prepare("SELECT space, subject_id, admin FROM zen_data_grants ORDER BY space, id")?;
    let rows = q.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)? == 1)))?;
    let mut out: Vec<(String, usize, Vec<String>)> = Vec::new();
    for x in rows {
        let (space, subject, admin) = x?;
        if out.last().map(|l| &l.0) != Some(&space) {
            out.push((space.clone(), 0, Vec::new()));
        }
        let l = out.last_mut().expect("pushed above");
        l.1 += 1;
        if admin {
            l.2.push(subject);
        }
    }
    Ok(out)
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

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        crate::db::run_migrations(&conn).unwrap();
        conn.execute(
            "INSERT INTO project_groups (id, name, created_at, updated_at) VALUES ('pg-q4', 'Q4 launch', 1, 1)",
            [],
        )
        .unwrap();
        conn
    }

    fn join(conn: &Connection, ws: &str) {
        conn.execute(
            "INSERT INTO project_group_members (group_id, workspace_id, created_at) VALUES ('pg-q4', ?1, 1)",
            [ws],
        )
        .unwrap();
    }

    fn board() -> Space {
        Space::data("board").unwrap()
    }

    fn new(prefix: &str, kind: SubjectKind, id: &str, level: Level) -> NewGrant {
        NewGrant {
            space: board(),
            prefix: prefix.into(),
            subject_kind: kind,
            subject_id: id.into(),
            level,
            admin: false,
            via_grant: None,
            granted_by: "owner".into(),
        }
    }

    #[test]
    fn direct_and_project_grants_resolve_live() {
        let conn = db();
        upsert(&conn, &new("shared/", SubjectKind::Workspace, "w-keeper", Level::Read), 5).unwrap();
        upsert(&conn, &new("project/7/", SubjectKind::Project, "pg-q4", Level::Write), 5).unwrap();
        let a = access_for_agent(&conn, &board(), "w-keeper").unwrap();
        assert!(a.can_read("shared/x") && !a.can_write("project/7/x"), "not a member yet");
        join(&conn, "w-keeper");
        let gs = effective_grants(&conn, &board(), "w-keeper").unwrap();
        assert_eq!(gs.len(), 2);
        assert_eq!(gs[0].from, GrantFrom::Project { id: "pg-q4".into(), name: "Q4 launch".into() });
        assert!(access_for_agent(&conn, &board(), "w-keeper").unwrap().can_write("project/7/board/1"));
        conn.execute("DELETE FROM project_group_members WHERE workspace_id = 'w-keeper'", []).unwrap();
        assert!(!access_for_agent(&conn, &board(), "w-keeper").unwrap().can_read("project/7/x"), "left the project");
        assert_eq!(access_for_agent(&conn, &board(), "w-other").unwrap(), Access::None);
        // Another space's grants never apply.
        assert_eq!(access_for_agent(&conn, &Space::data("diary").unwrap(), "w-keeper").unwrap(), Access::None);
    }

    #[test]
    fn upsert_updates_in_place_and_saves_are_never_granted() {
        let conn = db();
        let (r1, created) = upsert(&conn, &new("", SubjectKind::Workspace, "w-1", Level::Read), 5).unwrap();
        assert!(created);
        let (r2, created) = upsert(&conn, &new("", SubjectKind::Workspace, "w-1", Level::Write), 6).unwrap();
        assert!(!created);
        assert_eq!((r2.id, r2.level), (r1.id, Level::Write));
        let mut save = new("", SubjectKind::Workspace, "w-1", Level::Read);
        save.space = Space::save("board").unwrap();
        assert_eq!(upsert(&conn, &save, 5).unwrap_err().code(), "bad_request");
        assert_eq!(upsert(&conn, &new("/bad", SubjectKind::Workspace, "w-1", Level::Read), 5).unwrap_err().code(), "bad_key");
    }

    #[test]
    fn delegate_bounds() {
        let conn = db();
        let mut d = new("project/", SubjectKind::Workspace, "w-builder", Level::Write);
        d.admin = true;
        let (deleg, _) = upsert(&conn, &d, 5).unwrap();
        let ok = new("project/7/", SubjectKind::Workspace, "w-keeper", Level::Write);
        check_delegate_grant(&deleg, &ok, "w-builder").unwrap();
        check_delegate_grant(&deleg, &new("project/7/", SubjectKind::Project, "pg-q4", Level::Read), "w-builder").unwrap();
        let outside = new("other/", SubjectKind::Workspace, "w-keeper", Level::Read);
        assert_eq!(check_delegate_grant(&deleg, &outside, "w-builder").unwrap_err().code(), "not_granted");
        let whole = new("", SubjectKind::Workspace, "w-keeper", Level::Read);
        assert_eq!(check_delegate_grant(&deleg, &whole, "w-builder").unwrap_err().code(), "not_granted");
        let me = new("project/7/", SubjectKind::Workspace, "w-builder", Level::Read);
        assert_eq!(check_delegate_grant(&deleg, &me, "w-builder").unwrap_err().code(), "self_grant");
        let mut redeleg = ok.clone();
        redeleg.admin = true;
        assert_eq!(check_delegate_grant(&deleg, &redeleg, "w-builder").unwrap_err().code(), "not_granted");
        let mut reader = d.clone();
        reader.level = Level::Read;
        reader.subject_id = "w-reader".into();
        let (rd, _) = upsert(&conn, &reader, 5).unwrap();
        assert_eq!(check_delegate_grant(&rd, &ok, "w-reader").unwrap_err().code(), "level_too_high");
        // A plain grant isn't a delegation.
        let (plain, _) = upsert(&conn, &new("", SubjectKind::Workspace, "w-x", Level::Write), 5).unwrap();
        assert_eq!(check_delegate_grant(&plain, &ok, "w-x").unwrap_err().code(), "not_granted");
        // The delegate holds its own level on its prefix.
        let a = access_for_agent(&conn, &board(), "w-builder").unwrap();
        assert!(a.can_write("project/1/obj") && !a.can_read("elsewhere"));
        assert_eq!(delegations_of(&conn, &board(), "w-builder").unwrap().len(), 1);
    }

    #[test]
    fn revoking_a_delegation_cascades_and_deletes_clean_up() {
        let conn = db();
        let mut d = new("", SubjectKind::Workspace, "w-builder", Level::Write);
        d.admin = true;
        let (deleg, _) = upsert(&conn, &d, 5).unwrap();
        for ws in ["w-a", "w-b"] {
            let mut made = new("p/", SubjectKind::Workspace, ws, Level::Read);
            made.via_grant = Some(deleg.id);
            made.granted_by = "agent:w-builder".into();
            upsert(&conn, &made, 5).unwrap();
        }
        let (person, _) = upsert(&conn, &new("q/", SubjectKind::Workspace, "w-c", Level::Read), 5).unwrap();
        assert_eq!(made_under(&conn, deleg.id).unwrap(), 2);
        assert_eq!(list_made_under(&conn, deleg.id).unwrap().len(), 2);
        assert_eq!(remove(&conn, deleg.id).unwrap(), 2);
        assert_eq!(list_space(&conn, &board()).unwrap().len(), 1);
        assert_eq!(remove(&conn, deleg.id).unwrap_err().code(), "grant_not_found");
        assert!(by_id(&conn, person.id).unwrap().is_some());
        // Workspace delete takes its rows and what its delegations made.
        let (deleg, _) = upsert(&conn, &d, 5).unwrap();
        let mut made = new("p/", SubjectKind::Workspace, "w-a", Level::Read);
        made.via_grant = Some(deleg.id);
        made.granted_by = "agent:w-builder".into();
        upsert(&conn, &made, 5).unwrap();
        assert_eq!(delete_for_workspace(&conn, "w-builder").unwrap(), 2);
        upsert(&conn, &new("", SubjectKind::Project, "pg-q4", Level::Read), 5).unwrap();
        assert_eq!(delete_for_project_group(&conn, "pg-q4").unwrap(), 1);
        let (_, _) = upsert(&conn, &new("", SubjectKind::Workspace, "w-z", Level::Read), 5).unwrap();
        let to = Space::data("board2").unwrap();
        assert_eq!(move_space(&conn, &board(), &to).unwrap(), 2);
        let s = summary(&conn).unwrap();
        assert_eq!(s, vec![("data:board2".to_string(), 2, vec![])]);
        assert_eq!(delete_for_space(&conn, &to).unwrap(), 2);
    }
}
