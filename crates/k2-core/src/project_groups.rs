//! Projects V1 P1 (prd-projects-v1 §3) — named GROUPS of workspaces.
//!
//! NOT the legacy `projects` table (that IS the workspace registry —
//! see the PRD's §2b naming-collision note): a *project group* is a
//! named set of workspaces (a workspace IS an agent), with exactly one
//! Point of Contact (PoC) member, ONE chat stream, and canonical shared
//! dashboards. The daemon's `/cli/project-group/*` routes (P2) and the
//! `k2 project` CLI (P3) are thin wrappers over this module
//! (daemon-first: all logic lives here).
//!
//! Addressing: groups resolve by FULL id, EXACT name (case-insensitive
//! — the column is UNIQUE COLLATE NOCASE), or a UNIQUE NAME PREFIX; an
//! exact name match always beats prefix matching (see
//! [`resolve_group`]). Mutators take the FULL group id — callers
//! resolve first (the feedback.rs discipline).
//!
//! Error convention: stable-code errors carry the code as the FIRST
//! TOKEN of the `Err(String)`, `"<code>: <hint>"` — the P2 routes layer
//! splits on the first `": "` to build the wire
//! `{"error":{"code","hint"}}` shape. Codes used here: `name_taken`,
//! `poc_successor_required`, `not_a_member`.

use rusqlite::params;

/// The layout blob a fresh dashboard starts with (versioned; the
/// structure inside is renderer-owned — core only guarantees valid
/// JSON, see [`save_dashboard_layout`]).
pub const EMPTY_LAYOUT_V1: &str = r#"{"version":1,"panes":[]}"#;

/// The auto-created dashboard every group starts with (PRD §3: V1
/// surfaces exactly this one; multi-dashboard UI is V1.1).
pub const MAIN_DASHBOARD_NAME: &str = "Main";

/// One `project_groups` row + its member count. Serializes camelCase —
/// the wire shape the P2 routes return.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectGroup {
    pub id: String,
    pub name: String,
    /// `projects.id` of the PoC; `None` only while the group is
    /// memberless (first member auto-becomes PoC).
    pub poc_workspace_id: Option<String>,
    /// Canonical nav Pinned-section flag (resolved Q4).
    pub pinned: bool,
    /// Nav ordering within its section.
    pub sort_order: i64,
    pub created_at: i64,
    pub updated_at: i64,
    /// Membership size (nav member strip / list badges).
    pub member_count: i64,
}

/// One `project_group_members` row.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectGroupMember {
    pub group_id: String,
    /// `projects.id` (the workspace registry — no FK, see 0066).
    pub workspace_id: String,
    pub created_at: i64,
}

/// One `project_group_messages` row (the single per-group chat stream).
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectGroupMessage {
    pub id: String,
    pub group_id: String,
    /// `'owner'` | an agent/workspace name (feedback_comments.author
    /// convention). Membership enforcement (`not_a_member`) is
    /// route-level in P2 — routes map the author to a workspace via
    /// `workspace_msg::resolve_workspace`; core stores what it is told.
    pub author: String,
    pub body: String,
    pub created_at: i64,
}

/// One `project_group_dashboards` row.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectGroupDashboard {
    pub id: String,
    pub group_id: String,
    pub name: String,
    /// Versioned blob (§6.3); core validates parseability only.
    pub layout_json: String,
    /// Monotonic last-write-wins / staleness counter.
    pub revision: i64,
    pub position: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

/// [`add_member`]'s outcome. Re-adding an existing member is an Ok
/// NO-OP (`already_member = true`); the FIRST member of an empty group
/// auto-becomes the PoC (`became_poc = true`).
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AddMemberOutcome {
    pub member: ProjectGroupMember,
    pub already_member: bool,
    pub became_poc: bool,
}

/// A page of the chat stream, oldest-first. `truncated` = more rows
/// matched than the effective limit allowed (the caller should page).
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessagePage {
    pub messages: Vec<ProjectGroupMessage>,
    pub truncated: bool,
}

/// Group-resolution failure: either nothing matched the selector, or a
/// name prefix is ambiguous — the CLI turns both into exit 4, the
/// ambiguous arm listing the candidate NAMES in the hint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveError {
    NotFound,
    Ambiguous(Vec<String>),
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

const GROUP_SELECT: &str = "SELECT g.id, g.name, g.poc_workspace_id, g.pinned, \
    g.sort_order, g.created_at, g.updated_at, \
    (SELECT COUNT(*) FROM project_group_members m WHERE m.group_id = g.id) \
    FROM project_groups g";

fn row_to_group(row: &rusqlite::Row) -> rusqlite::Result<ProjectGroup> {
    Ok(ProjectGroup {
        id: row.get(0)?,
        name: row.get(1)?,
        poc_workspace_id: row.get(2)?,
        pinned: row.get::<_, i64>(3)? != 0,
        sort_order: row.get(4)?,
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
        member_count: row.get(7)?,
    })
}

const DASHBOARD_SELECT: &str = "SELECT id, group_id, name, layout_json, revision, \
    position, created_at, updated_at FROM project_group_dashboards";

fn row_to_dashboard(row: &rusqlite::Row) -> rusqlite::Result<ProjectGroupDashboard> {
    Ok(ProjectGroupDashboard {
        id: row.get(0)?,
        group_id: row.get(1)?,
        name: row.get(2)?,
        layout_json: row.get(3)?,
        revision: row.get(4)?,
        position: row.get(5)?,
        created_at: row.get(6)?,
        updated_at: row.get(7)?,
    })
}

fn row_to_message(row: &rusqlite::Row) -> rusqlite::Result<ProjectGroupMessage> {
    Ok(ProjectGroupMessage {
        id: row.get(0)?,
        group_id: row.get(1)?,
        author: row.get(2)?,
        body: row.get(3)?,
        created_at: row.get(4)?,
    })
}

/// True when a group with this name already exists (name is UNIQUE
/// COLLATE NOCASE — the check is case-insensitive via the column's
/// collation). `exclude_id` skips one row (rename-to-own-name).
fn name_exists(conn: &rusqlite::Connection, name: &str, exclude_id: Option<&str>) -> bool {
    conn.query_row(
        "SELECT COUNT(*) > 0 FROM project_groups WHERE name = ?1 AND id != ?2",
        params![name, exclude_id.unwrap_or("")],
        |row| row.get(0),
    )
    .unwrap_or(false)
}

/// Create a group + its auto 'Main' dashboard (position 0, empty
/// versioned layout blob). Name must be non-empty and unique
/// (case-insensitive) — a taken name fails with the stable
/// `name_taken` code.
pub fn create_group(name: &str) -> Result<ProjectGroup, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("usage: project name must not be empty".to_string());
    }
    let id = uuid::Uuid::new_v4().to_string();
    let now = now_secs();
    let db = crate::db::shared();
    let conn = db.lock();
    if name_exists(&conn, name, None) {
        return Err(format!("name_taken: a project named '{name}' already exists"));
    }
    conn.execute(
        "INSERT INTO project_groups (id, name, poc_workspace_id, pinned, sort_order, \
         created_at, updated_at) VALUES (?1, ?2, NULL, 0, 0, ?3, ?3)",
        params![id, name, now],
    )
    .map_err(|e| {
        // The UNIQUE COLLATE NOCASE constraint backstops a race between
        // the check above and the insert.
        if e.to_string().contains("UNIQUE") {
            format!("name_taken: a project named '{name}' already exists")
        } else {
            format!("project group insert failed: {e}")
        }
    })?;
    conn.execute(
        "INSERT INTO project_group_dashboards (id, group_id, name, layout_json, \
         revision, position, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, 0, 0, ?5, ?5)",
        params![
            uuid::Uuid::new_v4().to_string(),
            id,
            MAIN_DASHBOARD_NAME,
            EMPTY_LAYOUT_V1,
            now
        ],
    )
    .map_err(|e| format!("main dashboard insert failed: {e}"))?;
    drop(conn);
    get_group_by_id(&id).ok_or_else(|| "project group vanished after insert".to_string())
}

/// All groups (member counts + poc included), nav order: pinned first,
/// then sort_order, then name.
pub fn list_groups() -> Result<Vec<ProjectGroup>, String> {
    let db = crate::db::shared();
    let conn = db.lock();
    let sql = format!(
        "{GROUP_SELECT} ORDER BY g.pinned DESC, g.sort_order ASC, g.name COLLATE NOCASE ASC"
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| format!("prepare: {e}"))?;
    let rows = stmt.query_map([], row_to_group).map_err(|e| format!("query: {e}"))?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| format!("row: {e}"))
}

/// Fetch one group by FULL id. `None` when it doesn't exist.
pub fn get_group_by_id(id: &str) -> Option<ProjectGroup> {
    let db = crate::db::shared();
    let conn = db.lock();
    let sql = format!("{GROUP_SELECT} WHERE g.id = ?1");
    conn.query_row(&sql, params![id], row_to_group).ok()
}

/// Resolve a selector (full id | exact name | unique name prefix) to
/// the FULL group id.
///
/// Precedence — each tier only runs when the previous found nothing:
/// 1. FULL id match.
/// 2. EXACT name match (case-insensitive; the column collation). An
///    exact name ALWAYS beats prefix matching — a group named
///    `release` resolves even when `release-2` also exists.
/// 3. UNIQUE name prefix (case-insensitive): one match resolves; two
///    or more → [`ResolveError::Ambiguous`] carrying the candidate
///    NAMES (up to 10, for the CLI's did-you-mean hint).
pub fn resolve_group(selector: &str) -> Result<String, ResolveError> {
    let s = selector.trim();
    if s.is_empty() {
        return Err(ResolveError::NotFound);
    }
    let db = crate::db::shared();
    let conn = db.lock();
    // Tier 1: full id.
    if let Ok(id) = conn.query_row(
        "SELECT id FROM project_groups WHERE id = ?1",
        params![s],
        |row| row.get::<_, String>(0),
    ) {
        return Ok(id);
    }
    // Tier 2: exact name (NOCASE via the column collation).
    if let Ok(id) = conn.query_row(
        "SELECT id FROM project_groups WHERE name = ?1",
        params![s],
        |row| row.get::<_, String>(0),
    ) {
        return Ok(id);
    }
    // Tier 3: unique name prefix. Escape LIKE wildcards so a hostile
    // selector can't widen the match (feedback.rs idiom).
    let escaped = s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
    let mut stmt = match conn.prepare(
        "SELECT id, name FROM project_groups WHERE name LIKE ?1 ESCAPE '\\' \
         ORDER BY name COLLATE NOCASE ASC LIMIT 11",
    ) {
        Ok(st) => st,
        Err(_) => return Err(ResolveError::NotFound),
    };
    let matches: Vec<(String, String)> = match stmt.query_map(
        params![format!("{escaped}%")],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
    ) {
        Ok(rows) => rows.filter_map(Result::ok).collect(),
        Err(_) => return Err(ResolveError::NotFound),
    };
    match matches.len() {
        0 => Err(ResolveError::NotFound),
        1 => Ok(matches.into_iter().next().expect("len checked").0),
        _ => Err(ResolveError::Ambiguous(
            matches.into_iter().take(10).map(|(_, name)| name).collect(),
        )),
    }
}

/// Resolve a selector and fetch the group in one step (route `show`
/// convenience).
pub fn get_group(selector: &str) -> Result<ProjectGroup, ResolveError> {
    let id = resolve_group(selector)?;
    get_group_by_id(&id).ok_or(ResolveError::NotFound)
}

/// Rename a group. `id` must be a FULL id (callers resolve first). The
/// new name obeys the same non-empty + unique rules as [`create_group`]
/// (renaming a group to its own name — e.g. a case change — is fine).
pub fn rename_group(id: &str, new_name: &str) -> Result<ProjectGroup, String> {
    let new_name = new_name.trim();
    if new_name.is_empty() {
        return Err("usage: project name must not be empty".to_string());
    }
    let now = now_secs();
    let db = crate::db::shared();
    let conn = db.lock();
    if name_exists(&conn, new_name, Some(id)) {
        return Err(format!("name_taken: a project named '{new_name}' already exists"));
    }
    let updated = conn
        .execute(
            "UPDATE project_groups SET name = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, new_name, now],
        )
        .map_err(|e| format!("rename failed: {e}"))?;
    if updated == 0 {
        return Err(format!("no project group with id {id}"));
    }
    drop(conn);
    get_group_by_id(id).ok_or_else(|| "project group vanished after rename".to_string())
}

/// Set the canonical nav Pinned flag. `id` must be a FULL id.
pub fn set_pinned(id: &str, pinned: bool) -> Result<ProjectGroup, String> {
    let now = now_secs();
    let db = crate::db::shared();
    let conn = db.lock();
    let updated = conn
        .execute(
            "UPDATE project_groups SET pinned = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, pinned as i64, now],
        )
        .map_err(|e| format!("pin update failed: {e}"))?;
    if updated == 0 {
        return Err(format!("no project group with id {id}"));
    }
    drop(conn);
    get_group_by_id(id).ok_or_else(|| "project group vanished after pin".to_string())
}

/// Set the nav sort order within the group's section. `id` must be a
/// FULL id.
pub fn set_sort_order(id: &str, sort_order: i64) -> Result<ProjectGroup, String> {
    let now = now_secs();
    let db = crate::db::shared();
    let conn = db.lock();
    let updated = conn
        .execute(
            "UPDATE project_groups SET sort_order = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, sort_order, now],
        )
        .map_err(|e| format!("sort_order update failed: {e}"))?;
    if updated == 0 {
        return Err(format!("no project group with id {id}"));
    }
    drop(conn);
    get_group_by_id(id).ok_or_else(|| "project group vanished after reorder".to_string())
}

/// Delete a group. SQL CASCADE (foreign_keys=ON at connection open)
/// removes its members, messages, and dashboards — NEVER the workspaces
/// themselves (locked default, PRD ledger §11). `id` must be a FULL id.
pub fn delete_group(id: &str) -> Result<(), String> {
    let db = crate::db::shared();
    let conn = db.lock();
    let deleted = conn
        .execute("DELETE FROM project_groups WHERE id = ?1", params![id])
        .map_err(|e| format!("delete failed: {e}"))?;
    if deleted == 0 {
        return Err(format!("no project group with id {id}"));
    }
    Ok(())
}

/// A group's members, oldest-joined first.
pub fn list_members(group_id: &str) -> Result<Vec<ProjectGroupMember>, String> {
    let db = crate::db::shared();
    let conn = db.lock();
    let mut stmt = conn
        .prepare(
            "SELECT group_id, workspace_id, created_at FROM project_group_members \
             WHERE group_id = ?1 ORDER BY created_at ASC, rowid ASC",
        )
        .map_err(|e| format!("prepare: {e}"))?;
    let rows = stmt
        .query_map(params![group_id], |row| {
            Ok(ProjectGroupMember {
                group_id: row.get(0)?,
                workspace_id: row.get(1)?,
                created_at: row.get(2)?,
            })
        })
        .map_err(|e| format!("query: {e}"))?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| format!("row: {e}"))
}

fn is_member(conn: &rusqlite::Connection, group_id: &str, workspace_id: &str) -> bool {
    conn.query_row(
        "SELECT COUNT(*) > 0 FROM project_group_members \
         WHERE group_id = ?1 AND workspace_id = ?2",
        params![group_id, workspace_id],
        |row| row.get(0),
    )
    .unwrap_or(false)
}

/// Add a workspace to a group. `group_id` must be a FULL id.
///
/// - The FIRST member of an empty group auto-becomes the PoC (the
///   "first member auto-proposed" rule — `became_poc = true`).
/// - Re-adding an existing member is an Ok NO-OP
///   (`already_member = true`, nothing changes).
pub fn add_member(group_id: &str, workspace_id: &str) -> Result<AddMemberOutcome, String> {
    let workspace_id = workspace_id.trim();
    if workspace_id.is_empty() {
        return Err("usage: workspace_id must not be empty".to_string());
    }
    let now = now_secs();
    let db = crate::db::shared();
    let conn = db.lock();
    let poc: Option<String> = conn
        .query_row(
            "SELECT poc_workspace_id FROM project_groups WHERE id = ?1",
            params![group_id],
            |row| row.get(0),
        )
        .map_err(|_| format!("no project group with id {group_id}"))?;
    if is_member(&conn, group_id, workspace_id) {
        let member = conn
            .query_row(
                "SELECT group_id, workspace_id, created_at FROM project_group_members \
                 WHERE group_id = ?1 AND workspace_id = ?2",
                params![group_id, workspace_id],
                |row| {
                    Ok(ProjectGroupMember {
                        group_id: row.get(0)?,
                        workspace_id: row.get(1)?,
                        created_at: row.get(2)?,
                    })
                },
            )
            .map_err(|e| format!("member read failed: {e}"))?;
        return Ok(AddMemberOutcome { member, already_member: true, became_poc: false });
    }
    conn.execute(
        "INSERT INTO project_group_members (group_id, workspace_id, created_at) \
         VALUES (?1, ?2, ?3)",
        params![group_id, workspace_id, now],
    )
    .map_err(|e| format!("member insert failed: {e}"))?;
    // First member of an empty group auto-becomes the PoC.
    let became_poc = poc.is_none();
    if became_poc {
        conn.execute(
            "UPDATE project_groups SET poc_workspace_id = ?2, updated_at = ?3 WHERE id = ?1",
            params![group_id, workspace_id, now],
        )
        .map_err(|e| format!("auto-poc update failed: {e}"))?;
    } else {
        conn.execute(
            "UPDATE project_groups SET updated_at = ?2 WHERE id = ?1",
            params![group_id, now],
        )
        .map_err(|e| format!("group touch failed: {e}"))?;
    }
    Ok(AddMemberOutcome {
        member: ProjectGroupMember {
            group_id: group_id.to_string(),
            workspace_id: workspace_id.to_string(),
            created_at: now,
        },
        already_member: false,
        became_poc,
    })
}

/// Remove a workspace from a group. `group_id` must be a FULL id.
///
/// REFUSES to remove the current PoC (stable code
/// `poc_successor_required`) — [`set_poc`] must name a successor first
/// (no auto-reassignment, PRD resolved Q6). Removing a non-member fails
/// loudly with `not_a_member`.
pub fn remove_member(group_id: &str, workspace_id: &str) -> Result<(), String> {
    let now = now_secs();
    let db = crate::db::shared();
    let conn = db.lock();
    let poc: Option<String> = conn
        .query_row(
            "SELECT poc_workspace_id FROM project_groups WHERE id = ?1",
            params![group_id],
            |row| row.get(0),
        )
        .map_err(|_| format!("no project group with id {group_id}"))?;
    if poc.as_deref() == Some(workspace_id) {
        return Err(format!(
            "poc_successor_required: {workspace_id} is the Point of Contact — \
             reassign the PoC first (set_poc), then remove"
        ));
    }
    let removed = conn
        .execute(
            "DELETE FROM project_group_members WHERE group_id = ?1 AND workspace_id = ?2",
            params![group_id, workspace_id],
        )
        .map_err(|e| format!("member delete failed: {e}"))?;
    if removed == 0 {
        return Err(format!(
            "not_a_member: {workspace_id} is not a member of this project"
        ));
    }
    conn.execute(
        "UPDATE project_groups SET updated_at = ?2 WHERE id = ?1",
        params![group_id, now],
    )
    .map_err(|e| format!("group touch failed: {e}"))?;
    Ok(())
}

/// Reassign the PoC. `group_id` must be a FULL id; the target MUST
/// already be a member (`not_a_member` otherwise).
pub fn set_poc(group_id: &str, workspace_id: &str) -> Result<ProjectGroup, String> {
    let now = now_secs();
    let db = crate::db::shared();
    let conn = db.lock();
    if !is_member(&conn, group_id, workspace_id) {
        // Also covers a missing group (no members either way) — but
        // give the cleaner not-found error when the group is absent.
        let group_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM project_groups WHERE id = ?1",
                params![group_id],
                |row| row.get(0),
            )
            .unwrap_or(false);
        if !group_exists {
            return Err(format!("no project group with id {group_id}"));
        }
        return Err(format!(
            "not_a_member: {workspace_id} is not a member of this project — add it first"
        ));
    }
    conn.execute(
        "UPDATE project_groups SET poc_workspace_id = ?2, updated_at = ?3 WHERE id = ?1",
        params![group_id, workspace_id, now],
    )
    .map_err(|e| format!("poc update failed: {e}"))?;
    drop(conn);
    get_group_by_id(group_id).ok_or_else(|| "project group vanished after set_poc".to_string())
}

/// The current PoC workspace id (`None` only while memberless).
/// `group_id` must be a FULL id; a missing group errors.
pub fn get_poc(group_id: &str) -> Result<Option<String>, String> {
    let db = crate::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT poc_workspace_id FROM project_groups WHERE id = ?1",
        params![group_id],
        |row| row.get(0),
    )
    .map_err(|_| format!("no project group with id {group_id}"))
}

/// Every group NAME where this workspace is the PoC — the §4.5 removal
/// guard ("X is the Point of Contact for: A, B. Reassign the PoC
/// first."). P2 wires this into EVERY workspace-removal path
/// (workspace remove, agent retire). Empty vec = not a PoC anywhere,
/// removal may proceed.
pub fn poc_blocks_for_workspace(workspace_id: &str) -> Result<Vec<String>, String> {
    let db = crate::db::shared();
    let conn = db.lock();
    let mut stmt = conn
        .prepare(
            "SELECT name FROM project_groups WHERE poc_workspace_id = ?1 \
             ORDER BY name COLLATE NOCASE ASC",
        )
        .map_err(|e| format!("prepare: {e}"))?;
    let rows = stmt
        .query_map(params![workspace_id], |row| row.get::<_, String>(0))
        .map_err(|e| format!("query: {e}"))?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| format!("row: {e}"))
}

/// Post to the group's single chat stream. `group_id` must be a FULL
/// id. This layer STORES only — the P2 routes layer adds membership
/// enforcement (`not_a_member`, author→workspace via
/// `workspace_msg::resolve_workspace`), the `message-created` emit, and
/// the best-effort PoC injection (§4.3) on top.
pub fn post_message(
    group_id: &str,
    author: &str,
    body: &str,
) -> Result<ProjectGroupMessage, String> {
    let author = author.trim();
    let body_t = body.trim();
    if author.is_empty() {
        return Err("usage: author must not be empty".to_string());
    }
    if body_t.is_empty() {
        return Err("usage: message body must not be empty".to_string());
    }
    let now = now_secs();
    let id = uuid::Uuid::new_v4().to_string();
    let db = crate::db::shared();
    let conn = db.lock();
    // Check the parent explicitly so callers get a clean not-found
    // instead of an FK constraint error.
    let group_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM project_groups WHERE id = ?1",
            params![group_id],
            |row| row.get(0),
        )
        .unwrap_or(false);
    if !group_exists {
        return Err(format!("no project group with id {group_id}"));
    }
    conn.execute(
        "INSERT INTO project_group_messages (id, group_id, author, body, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![id, group_id, author, body_t, now],
    )
    .map_err(|e| format!("message insert failed: {e}"))?;
    Ok(ProjectGroupMessage {
        id,
        group_id: group_id.to_string(),
        author: author.to_string(),
        body: body_t.to_string(),
        created_at: now,
    })
}

/// With `after`, reads are capped at 500 rows per page.
pub const MESSAGES_AFTER_LIMIT: i64 = 500;
/// Without `after`, the default page is the latest 20 (drawer open).
pub const MESSAGES_DEFAULT_LIMIT: i64 = 20;

/// Read the chat stream, OLDEST-FIRST (rowid tiebreak for same-second
/// rows). `group_id` must be a FULL id.
///
/// - `after = None`: the LATEST `limit` messages (default 20, max 500),
///   returned oldest-first — the drawer-open tail. `truncated` = older
///   messages exist beyond the page.
/// - `after = Some(ts)`: messages with `created_at` STRICTLY GREATER
///   than `ts` (the CLI `read --after` / incremental-fetch path),
///   default AND max limit 500. `truncated` = more matching rows
///   remained past the cap — the caller should page again from the last
///   row's `created_at`.
pub fn list_messages(
    group_id: &str,
    after: Option<i64>,
    limit: Option<i64>,
) -> Result<MessagePage, String> {
    if let Some(l) = limit {
        if l < 1 {
            return Err("usage: limit must be >= 1".to_string());
        }
    }
    let db = crate::db::shared();
    let conn = db.lock();
    const MSG_COLS: &str = "id, group_id, author, body, created_at";
    match after {
        Some(ts) => {
            let eff = limit.unwrap_or(MESSAGES_AFTER_LIMIT).min(MESSAGES_AFTER_LIMIT);
            // Fetch one extra row to detect truncation.
            let sql = format!(
                "SELECT {MSG_COLS} FROM project_group_messages \
                 WHERE group_id = ?1 AND created_at > ?2 \
                 ORDER BY created_at ASC, rowid ASC LIMIT ?3"
            );
            let mut stmt = conn.prepare(&sql).map_err(|e| format!("prepare: {e}"))?;
            let mut messages: Vec<ProjectGroupMessage> = stmt
                .query_map(params![group_id, ts, eff + 1], row_to_message)
                .map_err(|e| format!("query: {e}"))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("row: {e}"))?;
            let truncated = messages.len() as i64 > eff;
            messages.truncate(eff as usize);
            Ok(MessagePage { messages, truncated })
        }
        None => {
            let eff = limit.unwrap_or(MESSAGES_DEFAULT_LIMIT).min(MESSAGES_AFTER_LIMIT);
            // Latest N: scan newest-first with one extra row for the
            // truncation probe, then reverse to oldest-first.
            let sql = format!(
                "SELECT {MSG_COLS} FROM project_group_messages WHERE group_id = ?1 \
                 ORDER BY created_at DESC, rowid DESC LIMIT ?2"
            );
            let mut stmt = conn.prepare(&sql).map_err(|e| format!("prepare: {e}"))?;
            let mut messages: Vec<ProjectGroupMessage> = stmt
                .query_map(params![group_id, eff + 1], row_to_message)
                .map_err(|e| format!("query: {e}"))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("row: {e}"))?;
            let truncated = messages.len() as i64 > eff;
            messages.truncate(eff as usize);
            messages.reverse();
            Ok(MessagePage { messages, truncated })
        }
    }
}

/// A group's dashboards, position order. V1 surfaces only the
/// auto-created 'Main' row; everything is dashboard-id-addressed so
/// V1.1 multi-dashboards needs no migration.
pub fn list_dashboards(group_id: &str) -> Result<Vec<ProjectGroupDashboard>, String> {
    let db = crate::db::shared();
    let conn = db.lock();
    let sql = format!(
        "{DASHBOARD_SELECT} WHERE group_id = ?1 ORDER BY position ASC, created_at ASC, rowid ASC"
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| format!("prepare: {e}"))?;
    let rows = stmt
        .query_map(params![group_id], row_to_dashboard)
        .map_err(|e| format!("query: {e}"))?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| format!("row: {e}"))
}

/// Fetch one dashboard by FULL id. `None` when it doesn't exist.
pub fn get_dashboard(dashboard_id: &str) -> Option<ProjectGroupDashboard> {
    let db = crate::db::shared();
    let conn = db.lock();
    let sql = format!("{DASHBOARD_SELECT} WHERE id = ?1");
    conn.query_row(&sql, params![dashboard_id], row_to_dashboard).ok()
}

/// Rename a dashboard (P8 — the §6.5 Settings Main-row rename; V1.1
/// create/delete/reorder land beside it). `dashboard_id` must be a
/// FULL id. The new name obeys the same non-empty rule as groups and
/// must be unique WITHIN the dashboard's group (the 0066
/// `UNIQUE (group_id, name)` constraint — `name_taken` on collision;
/// renaming a dashboard to its own name is a fine no-op). Never
/// touches `layout_json` / `revision` — a rename is metadata, not a
/// layout write.
pub fn rename_dashboard(
    dashboard_id: &str,
    new_name: &str,
) -> Result<ProjectGroupDashboard, String> {
    let new_name = new_name.trim();
    if new_name.is_empty() {
        return Err("usage: dashboard name must not be empty".to_string());
    }
    let now = now_secs();
    let db = crate::db::shared();
    let conn = db.lock();
    let group_id: String = conn
        .query_row(
            "SELECT group_id FROM project_group_dashboards WHERE id = ?1",
            params![dashboard_id],
            |row| row.get(0),
        )
        .map_err(|_| format!("no dashboard with id {dashboard_id}"))?;
    let taken: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM project_group_dashboards \
             WHERE group_id = ?1 AND name = ?2 AND id != ?3",
            params![group_id, new_name, dashboard_id],
            |row| row.get(0),
        )
        .unwrap_or(false);
    if taken {
        return Err(format!(
            "name_taken: a dashboard named '{new_name}' already exists in this project"
        ));
    }
    conn.execute(
        "UPDATE project_group_dashboards SET name = ?2, updated_at = ?3 WHERE id = ?1",
        params![dashboard_id, new_name, now],
    )
    .map_err(|e| {
        // The UNIQUE (group_id, name) constraint backstops a race
        // between the check above and the update.
        if e.to_string().contains("UNIQUE") {
            format!("name_taken: a dashboard named '{new_name}' already exists in this project")
        } else {
            format!("dashboard rename failed: {e}")
        }
    })?;
    drop(conn);
    get_dashboard(dashboard_id).ok_or_else(|| "dashboard vanished after rename".to_string())
}

/// Save a dashboard layout: LAST-WRITE-WINS, revision++ (the monotonic
/// staleness counter clients compare on `layout-changed`).
/// `dashboard_id` must be a FULL id. Core validates the blob is
/// PARSEABLE JSON only — the structure inside (§6.3 columns/panes) is
/// renderer-owned.
pub fn save_dashboard_layout(
    dashboard_id: &str,
    layout_json: &str,
) -> Result<ProjectGroupDashboard, String> {
    if let Err(e) = serde_json::from_str::<serde_json::Value>(layout_json) {
        return Err(format!("layout_json is not valid JSON: {e}"));
    }
    let now = now_secs();
    let db = crate::db::shared();
    let conn = db.lock();
    let updated = conn
        .execute(
            "UPDATE project_group_dashboards SET layout_json = ?2, \
             revision = revision + 1, updated_at = ?3 WHERE id = ?1",
            params![dashboard_id, layout_json, now],
        )
        .map_err(|e| format!("layout save failed: {e}"))?;
    if updated == 0 {
        return Err(format!("no dashboard with id {dashboard_id}"));
    }
    drop(conn);
    get_dashboard(dashboard_id).ok_or_else(|| "dashboard vanished after save".to_string())
}

// ──────────────────────────────────────────────────────────────────────
// Inline unit tests
// ──────────────────────────────────────────────────────────────────────
//
// `db::shared()` in tests is the PROCESS-GLOBAL in-memory DB shared by
// every test in the binary — each test uses its own unique group names
// / workspace ids (uuid-suffixed) so rows never collide. Prefix-
// resolution tests craft prefixes that INCLUDE the test's uuid, so they
// can never collide with sibling tests' rows (the feedback.rs
// shortest-prefix flake, avoided by construction).

#[cfg(test)]
mod tests {
    use super::*;

    /// A per-test unique group name: `<label>-<uuid>[-suffix]`.
    fn gname(label: &str) -> String {
        format!("{label}-{}", uuid::Uuid::new_v4())
    }

    fn wid(label: &str) -> String {
        format!("ws-{label}-{}", uuid::Uuid::new_v4())
    }

    #[test]
    fn create_defaults_main_dashboard_and_name_taken() {
        let name = gname("create");
        let g = create_group(&name).expect("create");
        assert_eq!(g.name, name);
        assert!(g.poc_workspace_id.is_none(), "memberless group has no PoC");
        assert!(!g.pinned);
        assert_eq!(g.sort_order, 0);
        assert_eq!(g.member_count, 0);
        assert!(g.created_at > 0);

        // The 'Main' dashboard is auto-created: position 0, versioned
        // empty layout, revision 0.
        let dashboards = list_dashboards(&g.id).expect("dashboards");
        assert_eq!(dashboards.len(), 1);
        let main = &dashboards[0];
        assert_eq!(main.name, MAIN_DASHBOARD_NAME);
        assert_eq!(main.position, 0);
        assert_eq!(main.revision, 0);
        assert_eq!(main.layout_json, EMPTY_LAYOUT_V1);
        let parsed: serde_json::Value =
            serde_json::from_str(&main.layout_json).expect("layout parses");
        assert_eq!(parsed["version"], 1);

        // Duplicate name → stable name_taken code; case-insensitive.
        let err = create_group(&name).expect_err("dup name must fail");
        assert!(err.starts_with("name_taken"), "got: {err}");
        let err = create_group(&name.to_uppercase()).expect_err("NOCASE dup must fail");
        assert!(err.starts_with("name_taken"), "got: {err}");

        // Empty/blank names are rejected.
        assert!(create_group("  ").is_err());
    }

    #[test]
    fn resolve_exact_name_beats_prefix() {
        // `base` is ALSO a strict prefix of `base-two`: the exact match
        // must win, never Ambiguous. Both names embed this test's uuid,
        // so no sibling test's rows can interfere.
        let base = gname("exact");
        let longer = format!("{base}-two");
        let g_base = create_group(&base).expect("create base");
        let g_longer = create_group(&longer).expect("create longer");

        assert_eq!(resolve_group(&base), Ok(g_base.id.clone()));
        // Case-insensitive exact match too.
        assert_eq!(resolve_group(&base.to_uppercase()), Ok(g_base.id.clone()));
        // Full id resolves to itself.
        assert_eq!(resolve_group(&g_base.id), Ok(g_base.id.clone()));
        // A prefix unique to the LONGER name resolves it.
        let uniq_prefix = format!("{base}-t");
        assert_eq!(resolve_group(&uniq_prefix), Ok(g_longer.id.clone()));
        // get_group round-trips the selector.
        let fetched = get_group(&base).expect("get_group by name");
        assert_eq!(fetched.id, g_base.id);
    }

    #[test]
    fn resolve_ambiguous_and_not_found() {
        let base = gname("ambig");
        let a = format!("{base}-alpha");
        let b = format!("{base}-beta");
        create_group(&a).expect("create a");
        create_group(&b).expect("create b");

        // `base-` prefixes BOTH (and only these two — the uuid in
        // `base` isolates this test from siblings) and matches neither
        // exactly → Ambiguous, candidates = the names.
        match resolve_group(&format!("{base}-")) {
            Err(ResolveError::Ambiguous(candidates)) => {
                assert_eq!(candidates.len(), 2, "both candidates listed: {candidates:?}");
                assert!(candidates.contains(&a));
                assert!(candidates.contains(&b));
            }
            other => panic!("expected Ambiguous, got {other:?}"),
        }

        assert_eq!(
            resolve_group(&format!("zzz-no-such-{}", uuid::Uuid::new_v4())),
            Err(ResolveError::NotFound)
        );
        assert_eq!(resolve_group("  "), Err(ResolveError::NotFound));
        // LIKE wildcards must not widen matching: an unescaped `_`
        // would match `{base}-alpha`; escaped, it matches nothing.
        assert_eq!(resolve_group("%"), Err(ResolveError::NotFound));
        assert_eq!(resolve_group(&format!("{base}_alpha")), Err(ResolveError::NotFound));
    }

    #[test]
    fn first_member_auto_poc_and_readd_noop() {
        let g = create_group(&gname("members")).expect("create");
        let w1 = wid("m1");
        let w2 = wid("m2");

        // First member auto-becomes PoC.
        let out = add_member(&g.id, &w1).expect("add first");
        assert!(!out.already_member);
        assert!(out.became_poc, "first member auto-becomes PoC");
        assert_eq!(get_poc(&g.id).expect("poc"), Some(w1.clone()));

        // Second member does NOT steal the PoC.
        let out2 = add_member(&g.id, &w2).expect("add second");
        assert!(!out2.already_member);
        assert!(!out2.became_poc);
        assert_eq!(get_poc(&g.id).expect("poc"), Some(w1.clone()));

        // Re-add is an Ok NO-OP.
        let readd = add_member(&g.id, &w1).expect("re-add");
        assert!(readd.already_member, "re-add reports already_member");
        assert!(!readd.became_poc);
        let members = list_members(&g.id).expect("members");
        assert_eq!(members.len(), 2, "re-add must not duplicate the row");
        assert_eq!(get_group_by_id(&g.id).expect("get").member_count, 2);

        // Unknown group fails loudly.
        assert!(add_member("nope", &w1).is_err());
        assert!(add_member(&g.id, "  ").is_err());
    }

    #[test]
    fn poc_removal_refused_until_successor_named() {
        let g = create_group(&gname("pocguard")).expect("create");
        let w1 = wid("poc");
        let w2 = wid("succ");
        add_member(&g.id, &w1).expect("add poc");
        add_member(&g.id, &w2).expect("add succ");

        // Removing the PoC is refused with the stable code.
        let err = remove_member(&g.id, &w1).expect_err("PoC removal must be refused");
        assert!(err.starts_with("poc_successor_required"), "got: {err}");
        assert_eq!(list_members(&g.id).expect("members").len(), 2, "nothing removed");

        // Name a successor, then removal succeeds.
        let updated = set_poc(&g.id, &w2).expect("set_poc successor");
        assert_eq!(updated.poc_workspace_id.as_deref(), Some(w2.as_str()));
        remove_member(&g.id, &w1).expect("remove ex-PoC after reassignment");
        let members = list_members(&g.id).expect("members");
        assert_eq!(members.len(), 1);
        assert_eq!(members[0].workspace_id, w2);
        assert_eq!(get_poc(&g.id).expect("poc"), Some(w2.clone()));

        // Removing a non-member fails loudly with not_a_member.
        let err = remove_member(&g.id, &w1).expect_err("gone already");
        assert!(err.starts_with("not_a_member"), "got: {err}");
        assert!(remove_member("nope", &w2).is_err(), "unknown group fails loudly");
    }

    #[test]
    fn set_poc_requires_membership() {
        let g = create_group(&gname("pocmember")).expect("create");
        let member = wid("in");
        let stranger = wid("out");
        add_member(&g.id, &member).expect("add");

        let err = set_poc(&g.id, &stranger).expect_err("non-member cannot be PoC");
        assert!(err.starts_with("not_a_member"), "got: {err}");
        assert_eq!(get_poc(&g.id).expect("poc"), Some(member.clone()), "PoC unchanged");

        assert!(set_poc("nope", &member).is_err(), "unknown group fails loudly");
        assert!(get_poc("nope").is_err(), "unknown group fails loudly");
    }

    #[test]
    fn poc_blocks_for_workspace_across_groups() {
        let w = wid("blocker");
        let other = wid("bystander");
        // PoC of TWO groups, plain member of a third.
        let name_a = gname("block-a");
        let name_b = gname("block-b");
        let g_a = create_group(&name_a).expect("a");
        let g_b = create_group(&name_b).expect("b");
        let g_c = create_group(&gname("block-c")).expect("c");
        add_member(&g_a.id, &w).expect("poc of a");
        add_member(&g_b.id, &w).expect("poc of b");
        add_member(&g_c.id, &other).expect("other is poc of c");
        add_member(&g_c.id, &w).expect("w plain member of c");

        let mut blocks = poc_blocks_for_workspace(&w).expect("blocks");
        blocks.sort();
        let mut expected = vec![name_a.clone(), name_b.clone()];
        expected.sort();
        assert_eq!(blocks, expected, "PoC groups only — plain membership never blocks");

        // Reassign one away → only the other remains.
        add_member(&g_a.id, &other).expect("successor joins a");
        set_poc(&g_a.id, &other).expect("reassign a");
        assert_eq!(poc_blocks_for_workspace(&w).expect("blocks"), vec![name_b]);

        // A workspace that is PoC nowhere → empty (removal may proceed).
        assert!(poc_blocks_for_workspace(&wid("free")).expect("blocks").is_empty());
    }

    #[test]
    fn messages_ordering_after_limit_truncated() {
        let g = create_group(&gname("chat")).expect("create");

        // Basic post + validation.
        let m1 = post_message(&g.id, "owner", "hello team").expect("post");
        assert_eq!(m1.author, "owner");
        assert_eq!(m1.body, "hello team");
        assert!(post_message(&g.id, "  ", "x").is_err());
        assert!(post_message(&g.id, "owner", "  ").is_err());
        assert!(post_message("nope", "owner", "x").is_err(), "unknown group fails loudly");

        // Controlled timestamps for the paging math: a dedicated group
        // with 30 direct-inserted rows at created_at 1000..=1029.
        let paged = create_group(&gname("chat-paged")).expect("create paged");
        {
            let db = crate::db::shared();
            let conn = db.lock();
            for i in 0..30i64 {
                conn.execute(
                    "INSERT INTO project_group_messages (id, group_id, author, body, created_at) \
                     VALUES (?1, ?2, 'scout', ?3, ?4)",
                    params![
                        uuid::Uuid::new_v4().to_string(),
                        paged.id,
                        format!("msg-{i}"),
                        1000 + i
                    ],
                )
                .expect("insert message");
            }
        }

        // No `after`: the LATEST 20 (default), oldest-first, truncated
        // because 10 older rows exist.
        let page = list_messages(&paged.id, None, None).expect("default page");
        assert_eq!(page.messages.len(), 20);
        assert!(page.truncated, "10 older messages remained");
        assert_eq!(page.messages.first().expect("first").created_at, 1010);
        assert_eq!(page.messages.last().expect("last").created_at, 1029);
        let times: Vec<i64> = page.messages.iter().map(|m| m.created_at).collect();
        let mut sorted = times.clone();
        sorted.sort();
        assert_eq!(times, sorted, "oldest-first");

        // No `after`, explicit limit: latest 5.
        let page = list_messages(&paged.id, None, Some(5)).expect("limit 5");
        assert_eq!(page.messages.len(), 5);
        assert!(page.truncated);
        assert_eq!(page.messages.first().expect("first").created_at, 1025);

        // `after` is STRICTLY greater: after=1009 → 1010..=1029 (20
        // rows), NOT truncated (nothing matching remained).
        let page = list_messages(&paged.id, Some(1009), None).expect("after");
        assert_eq!(page.messages.len(), 20);
        assert!(!page.truncated);
        assert_eq!(page.messages.first().expect("first").created_at, 1010);
        // after=1010 excludes the ts itself.
        let page = list_messages(&paged.id, Some(1010), None).expect("after strict");
        assert_eq!(page.messages.first().expect("first").created_at, 1011);

        // `after` + limit smaller than the remainder → truncated.
        let page = list_messages(&paged.id, Some(999), Some(10)).expect("after limited");
        assert_eq!(page.messages.len(), 10);
        assert!(page.truncated, "20 matching rows remained");
        assert_eq!(page.messages.first().expect("first").created_at, 1000);
        assert_eq!(page.messages.last().expect("last").created_at, 1009);

        // `after` past everything → empty, not truncated.
        let page = list_messages(&paged.id, Some(99999), None).expect("after end");
        assert!(page.messages.is_empty());
        assert!(!page.truncated);

        // Bad limit fails loudly.
        assert!(list_messages(&paged.id, None, Some(0)).is_err());

        // Same-second posts keep insertion order (rowid tiebreak).
        let m2 = post_message(&g.id, "scout", "second").expect("post 2");
        let page = list_messages(&g.id, None, None).expect("page");
        let ids: Vec<&str> = page.messages.iter().map(|m| m.id.as_str()).collect();
        let pos1 = ids.iter().position(|i| *i == m1.id).expect("m1 present");
        let pos2 = ids.iter().position(|i| *i == m2.id).expect("m2 present");
        assert!(pos1 < pos2, "insertion order within the same second");
    }

    #[test]
    fn dashboard_save_layout_revision_and_json_validation() {
        let g = create_group(&gname("dash")).expect("create");
        let main = list_dashboards(&g.id).expect("dashboards").remove(0);
        assert_eq!(get_dashboard(&main.id).expect("get").id, main.id);

        // Valid save: blob replaced, revision bumps (last-write-wins).
        let layout =
            r#"{"version":1,"columns":[{"widthPct":100,"pane":{"kind":"terminal","workspaceId":"w1"}}]}"#;
        let saved = save_dashboard_layout(&main.id, layout).expect("save");
        assert_eq!(saved.layout_json, layout);
        assert_eq!(saved.revision, 1);
        let saved2 = save_dashboard_layout(&main.id, EMPTY_LAYOUT_V1).expect("save again");
        assert_eq!(saved2.revision, 2, "revision is monotonic");
        assert_eq!(saved2.layout_json, EMPTY_LAYOUT_V1, "last write wins");

        // Unparseable JSON is rejected; the stored blob is untouched.
        assert!(save_dashboard_layout(&main.id, "{not json").is_err());
        assert_eq!(
            get_dashboard(&main.id).expect("get").layout_json,
            EMPTY_LAYOUT_V1,
            "failed save must not corrupt the stored layout"
        );

        // Unknown dashboard fails loudly.
        assert!(save_dashboard_layout("nope", EMPTY_LAYOUT_V1).is_err());
        assert!(get_dashboard("nope").is_none());
    }

    #[test]
    fn rename_dashboard_rules() {
        let g = create_group(&gname("dashname")).expect("create");
        let main = list_dashboards(&g.id).expect("dashboards").remove(0);

        // Plain rename: name + updated_at move; layout/revision don't.
        let renamed = rename_dashboard(&main.id, "Release war room").expect("rename");
        assert_eq!(renamed.name, "Release war room");
        assert_eq!(renamed.revision, main.revision, "rename must not bump revision");
        assert_eq!(renamed.layout_json, main.layout_json, "rename must not touch the layout");
        assert_eq!(get_dashboard(&main.id).expect("get").name, "Release war room");

        // Whitespace trims; empty/blank is refused loudly.
        let renamed = rename_dashboard(&main.id, "  Ops  ").expect("trimmed rename");
        assert_eq!(renamed.name, "Ops");
        assert!(rename_dashboard(&main.id, "   ").is_err());

        // Renaming to its OWN name is a fine no-op.
        let same = rename_dashboard(&main.id, "Ops").expect("own-name rename");
        assert_eq!(same.name, "Ops");

        // A sibling dashboard in the SAME group can't take the name —
        // stable name_taken code (UNIQUE (group_id, name), 0066).
        {
            let db = crate::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO project_group_dashboards (id, group_id, name, layout_json, \
                 revision, position, created_at, updated_at) \
                 VALUES (?1, ?2, 'Second', ?3, 0, 1, 1000, 1000)",
                params![uuid::Uuid::new_v4().to_string(), g.id, EMPTY_LAYOUT_V1],
            )
            .expect("insert sibling dashboard");
        }
        let err = rename_dashboard(&main.id, "Second").expect_err("dup within group refused");
        assert!(err.starts_with("name_taken"), "got: {err}");
        assert_eq!(get_dashboard(&main.id).expect("get").name, "Ops", "refusal changes nothing");

        // The same name in ANOTHER group is fine (uniqueness is
        // per-group).
        let other = create_group(&gname("dashname-other")).expect("create other");
        let other_main = list_dashboards(&other.id).expect("dashboards").remove(0);
        assert_eq!(rename_dashboard(&other_main.id, "Ops").expect("cross-group ok").name, "Ops");

        // Unknown id fails loudly.
        assert!(rename_dashboard("nope", "x").is_err());
    }

    #[test]
    fn rename_pin_sort_order() {
        let g = create_group(&gname("meta")).expect("create");
        let taken = gname("meta-taken");
        create_group(&taken).expect("create taken");

        // Rename works; renaming ONTO a taken name → name_taken.
        let new_name = gname("meta-renamed");
        let renamed = rename_group(&g.id, &new_name).expect("rename");
        assert_eq!(renamed.name, new_name);
        assert_eq!(resolve_group(&new_name), Ok(g.id.clone()));
        let err = rename_group(&g.id, &taken).expect_err("taken name refused");
        assert!(err.starts_with("name_taken"), "got: {err}");
        // Case-change rename of one's OWN name is allowed.
        let recased = rename_group(&g.id, &new_name.to_uppercase()).expect("own-name recase");
        assert_eq!(recased.name, new_name.to_uppercase());
        assert!(rename_group(&g.id, " ").is_err());

        // Pin + sort order round-trip.
        let pinned = set_pinned(&g.id, true).expect("pin");
        assert!(pinned.pinned);
        let ordered = set_sort_order(&g.id, 7).expect("sort");
        assert_eq!(ordered.sort_order, 7);
        let unpinned = set_pinned(&g.id, false).expect("unpin");
        assert!(!unpinned.pinned);
        assert_eq!(unpinned.sort_order, 7, "sort_order survives unpin");

        // Unknown ids fail loudly everywhere.
        assert!(rename_group("nope", "x").is_err());
        assert!(set_pinned("nope", true).is_err());
        assert!(set_sort_order("nope", 1).is_err());

        // list_groups carries the group with its metadata.
        let all = list_groups().expect("list");
        let mine = all.iter().find(|x| x.id == g.id).expect("group listed");
        assert_eq!(mine.sort_order, 7);
    }

    #[test]
    fn delete_cascades_group_rows_never_workspaces() {
        // Seed a LEGACY workspace-registry row (`projects`) and a legacy
        // `workspaces` row so "never touches workspaces" is a real
        // assertion, not a 0 == 0 tautology.
        let legacy_project = wid("legacy");
        let legacy_workspace = format!("wt-{}", uuid::Uuid::new_v4());
        {
            let db = crate::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO projects (id, name, path) VALUES (?1, 'legacy', ?2)",
                params![legacy_project, format!("/tmp/{legacy_project}")],
            )
            .expect("seed legacy project");
            conn.execute(
                "INSERT INTO workspaces (id, project_id, name) VALUES (?1, ?2, 'main')",
                params![legacy_workspace, legacy_project],
            )
            .expect("seed legacy workspace");
        }

        let g = create_group(&gname("cascade")).expect("create");
        add_member(&g.id, &legacy_project).expect("add member");
        add_member(&g.id, &wid("extra")).expect("add extra");
        post_message(&g.id, "owner", "kickoff").expect("msg");
        post_message(&g.id, "scout", "ack").expect("msg 2");
        assert_eq!(list_dashboards(&g.id).expect("dash").len(), 1);

        delete_group(&g.id).expect("delete");

        // Every dependent row is gone.
        let db = crate::db::shared();
        let conn = db.lock();
        let count = |sql: &str| -> i64 {
            conn.query_row(sql, params![g.id], |row| row.get(0)).expect("count")
        };
        assert!(get_group_by_id(&g.id).is_none());
        assert_eq!(count("SELECT COUNT(*) FROM project_group_members WHERE group_id = ?1"), 0);
        assert_eq!(count("SELECT COUNT(*) FROM project_group_messages WHERE group_id = ?1"), 0);
        assert_eq!(count("SELECT COUNT(*) FROM project_group_dashboards WHERE group_id = ?1"), 0);

        // The legacy tables are untouched: the member workspace's
        // registry row AND the legacy `workspaces` row both survive.
        let survives: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM projects WHERE id = ?1",
                params![legacy_project],
                |row| row.get(0),
            )
            .expect("count projects");
        assert_eq!(survives, 1, "delete_group must never touch the workspace registry");
        let survives_wt: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM workspaces WHERE id = ?1",
                params![legacy_workspace],
                |row| row.get(0),
            )
            .expect("count workspaces");
        assert_eq!(survives_wt, 1, "delete_group must never touch legacy workspaces rows");
        drop(conn);

        // Deleting again fails loudly.
        assert!(delete_group(&g.id).is_err());
    }
}
