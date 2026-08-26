//! Per-workspace compose-bar send history.
//!
//! Last 50 successful compose-bar lines, keyed by `projects.id`.
//! Written after `POST /cli/terminal/send-message` and after
//! `POST /cli/thread/post` with `via=compose` (same Message-the-agent bar).
//! Tickets share `send_message_to_session` and must not land here.

use rusqlite::params;
use serde::Serialize;
use uuid::Uuid;

use crate::db;

pub const COMPOSE_SEND_HISTORY_CAP: usize = 50;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ComposeSendHistoryEntry {
    pub id: String,
    pub body: String,
    pub author: String,
    pub created_at: i64,
}

fn normalize_fs_path(path: &str) -> String {
    let trimmed = path.trim();
    if trimmed == "/" {
        return "/".to_string();
    }
    trimmed.trim_end_matches('/').to_string()
}

/// Longest registered `projects.path` that is an exact match or a
/// parent prefix of `path` (`/x/foo` never matches `/x/foobar`).
pub fn resolve_project_id_for_path(path: &str) -> Option<String> {
    let path = normalize_fs_path(path);
    if path.is_empty() {
        return None;
    }
    let db = db::shared();
    let conn = db.lock();
    let mut stmt = conn.prepare("SELECT id, path FROM projects").ok()?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .ok()?;
    let mut best: Option<(String, usize)> = None;
    for row in rows.flatten() {
        let root = normalize_fs_path(&row.1);
        if root.is_empty() {
            continue;
        }
        let is_match = path == root || path.starts_with(&format!("{root}/"));
        if !is_match {
            continue;
        }
        let take = match &best {
            None => true,
            Some((_, cur_len)) => root.len() > *cur_len,
        };
        if take {
            best = Some((row.0, root.len()));
        }
    }
    best.map(|(id, _)| id)
}

pub fn project_path_for_id(project_id: &str) -> Option<String> {
    if project_id.is_empty() {
        return None;
    }
    let db = db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT path FROM projects WHERE id = ?1",
        params![project_id],
        |row| row.get::<_, String>(0),
    )
    .ok()
}

pub fn project_exists(project_id: &str) -> bool {
    if project_id.is_empty() {
        return false;
    }
    let db = db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT 1 FROM projects WHERE id = ?1",
        params![project_id],
        |_| Ok(()),
    )
    .is_ok()
}

/// Insert `body` for `project_id` and trim to the newest 50 in the
/// same transaction. Empty/whitespace bodies are not stored.
/// Consecutive duplicates **are** stored.
pub fn record_compose_send(
    project_id: &str,
    body: &str,
    author: &str,
) -> Result<Option<String>, String> {
    let body = body.trim();
    if body.is_empty() {
        return Ok(None);
    }
    if project_id.is_empty() || !project_exists(project_id) {
        return Ok(None);
    }
    let id = Uuid::new_v4().to_string();
    let db = db::shared();
    let conn = db.lock();
    conn.execute_batch("BEGIN")
        .map_err(|e| format!("begin compose-history tx: {e}"))?;
    let insert = conn.execute(
        "INSERT INTO workspace_compose_send_history (id, project_id, body, author)
         VALUES (?1, ?2, ?3, ?4)",
        params![id, project_id, body, author],
    );
    if let Err(e) = insert {
        let _ = conn.execute_batch("ROLLBACK");
        return Err(e.to_string());
    }
    // Nested SELECT: SQLite cannot DELETE from a table while the same
    // table is named in a simple NOT IN subquery.
    let trim = conn.execute(
        "DELETE FROM workspace_compose_send_history
         WHERE project_id = ?1
           AND id NOT IN (
             SELECT id FROM (
               SELECT id FROM workspace_compose_send_history
                WHERE project_id = ?1
                ORDER BY created_at DESC, rowid DESC
                LIMIT 50
             )
           )",
        params![project_id],
    );
    if let Err(e) = trim {
        let _ = conn.execute_batch("ROLLBACK");
        return Err(e.to_string());
    }
    conn.execute_batch("COMMIT")
        .map_err(|e| format!("commit compose-history tx: {e}"))?;
    Ok(Some(id))
}

/// Resolve `cwd` with longest `projects.path` prefix, then record.
/// Unknown path: skip insert (`Ok(None)`).
pub fn record_compose_send_for_cwd(
    cwd: &str,
    body: &str,
    author: &str,
) -> Result<Option<String>, String> {
    let body = body.trim();
    if body.is_empty() {
        return Ok(None);
    }
    let Some(project_id) = resolve_project_id_for_path(cwd) else {
        return Ok(None);
    };
    record_compose_send(&project_id, body, author)
}

pub fn list_compose_send_history(project_id: &str) -> Result<Vec<ComposeSendHistoryEntry>, String> {
    if project_id.is_empty() {
        return Ok(Vec::new());
    }
    let db = db::shared();
    let conn = db.lock();
    let mut stmt = conn
        .prepare(
            "SELECT id, body, author, created_at
               FROM workspace_compose_send_history
              WHERE project_id = ?1
              ORDER BY created_at DESC, rowid DESC
              LIMIT 50",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![project_id], |row| {
            Ok(ComposeSendHistoryEntry {
                id: row.get(0)?,
                body: row.get(1)?,
                author: row.get(2)?,
                created_at: row.get(3)?,
            })
        })
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

/// `project_id` wins when the row exists; otherwise longest-prefix
/// `workspace_path`. Unknown → empty list.
pub fn list_for_query(
    project_id: &str,
    workspace_path: &str,
) -> Result<Vec<ComposeSendHistoryEntry>, String> {
    if !project_id.is_empty() {
        if project_exists(project_id) {
            return list_compose_send_history(project_id);
        }
        return Ok(Vec::new());
    }
    match resolve_project_id_for_path(workspace_path) {
        Some(id) => list_compose_send_history(&id),
        None => Ok(Vec::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ensure_project(id: &str, path: &str, name: &str) {
        let db = db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT OR REPLACE INTO projects \
             (id, path, name, color, agent_mode, pinned, tab_order) \
             VALUES (?1, ?2, ?3, '#123456', 'off', 0, 0)",
            params![id, path, name],
        )
        .expect("insert project");
    }

    fn delete_project(id: &str) {
        let db = db::shared();
        let conn = db.lock();
        conn.execute("DELETE FROM projects WHERE id = ?1", params![id])
            .expect("delete project");
    }

    fn count_for(project_id: &str) -> i64 {
        let db = db::shared();
        let conn = db.lock();
        conn.query_row(
            "SELECT COUNT(*) FROM workspace_compose_send_history WHERE project_id = ?1",
            params![project_id],
            |row| row.get(0),
        )
        .expect("count history")
    }

    fn unique(label: &str) -> String {
        format!("csh-{label}-{}-{}", std::process::id(), Uuid::new_v4())
    }

    #[test]
    fn empty_or_whitespace_body_is_not_stored() {
        db::init_for_tests();
        let pid = unique("empty");
        let path = format!("/tmp/{pid}");
        ensure_project(&pid, &path, "empty");
        let inserted = record_compose_send(&pid, "   \n\t  ", "owner").expect("record empty");
        assert!(
            inserted.is_none(),
            "whitespace body must not insert: {inserted:?}"
        );
        let inserted = record_compose_send(&pid, "", "owner").expect("record blank");
        assert!(inserted.is_none(), "empty body must not insert");
        assert_eq!(count_for(&pid), 0, "no rows after empty/whitespace");
        delete_project(&pid);
    }

    #[test]
    fn fifty_one_inserts_trim_to_fifty_oldest_gone() {
        db::init_for_tests();
        let pid = unique("cap");
        let path = format!("/tmp/{pid}");
        ensure_project(&pid, &path, "cap");
        for i in 0..51 {
            let body = format!("line-{i}");
            record_compose_send(&pid, &body, "owner")
                .expect("insert")
                .unwrap_or_else(|| panic!("insert {i} must store a row"));
        }
        let n = count_for(&pid);
        assert_eq!(n, 50, "cap must be 50 after 51 inserts, got {n}");
        let items = list_compose_send_history(&pid).expect("list");
        assert_eq!(items.len(), 50, "list must return 50, got {}", items.len());
        assert_eq!(
            items[0].body, "line-50",
            "newest must be first, got {:?}",
            items[0].body
        );
        assert!(
            items.iter().all(|e| e.body != "line-0"),
            "oldest line-0 must be trimmed: {:?}",
            items.iter().map(|e| e.body.as_str()).collect::<Vec<_>>()
        );
        assert_eq!(
            items[49].body, "line-1",
            "oldest remaining must be line-1, got {:?}",
            items[49].body
        );
        delete_project(&pid);
    }

    #[test]
    fn list_is_newest_first() {
        db::init_for_tests();
        let pid = unique("order");
        let path = format!("/tmp/{pid}");
        ensure_project(&pid, &path, "order");
        record_compose_send(&pid, "first", "a")
            .expect("first")
            .expect("stored");
        record_compose_send(&pid, "second", "b")
            .expect("second")
            .expect("stored");
        record_compose_send(&pid, "third", "c")
            .expect("third")
            .expect("stored");
        let items = list_compose_send_history(&pid).expect("list");
        assert_eq!(
            items.iter().map(|e| e.body.as_str()).collect::<Vec<_>>(),
            vec!["third", "second", "first"],
            "GET must be newest-first"
        );
        delete_project(&pid);
    }

    #[test]
    fn consecutive_duplicates_are_stored() {
        db::init_for_tests();
        let pid = unique("dup");
        let path = format!("/tmp/{pid}");
        ensure_project(&pid, &path, "dup");
        record_compose_send(&pid, "same", "owner")
            .expect("1")
            .expect("stored");
        record_compose_send(&pid, "same", "owner")
            .expect("2")
            .expect("stored");
        assert_eq!(count_for(&pid), 2, "consecutive dupes must both persist");
        delete_project(&pid);
    }

    #[test]
    fn worktree_cwd_maps_to_parent_project() {
        db::init_for_tests();
        let pid = unique("wt");
        let parent = format!("/tmp/{pid}-repo");
        ensure_project(&pid, &parent, "repo");
        let cwd = format!("{parent}/.worktrees/feature-x/src");
        let resolved = resolve_project_id_for_path(&cwd);
        assert_eq!(
            resolved.as_deref(),
            Some(pid.as_str()),
            "worktree cwd must map to parent project, got {resolved:?}"
        );
        record_compose_send_for_cwd(&cwd, "from-worktree", "owner")
            .expect("record cwd")
            .expect("stored");
        let items = list_compose_send_history(&pid).expect("list");
        assert_eq!(items.len(), 1, "parent project must own the worktree send");
        assert_eq!(items[0].body, "from-worktree");
        // Longer prefix wins when a nested project exists.
        let child_id = unique("child");
        let child_path = format!("{parent}/nested");
        ensure_project(&child_id, &child_path, "nested");
        let nested_cwd = format!("{child_path}/lib");
        assert_eq!(
            resolve_project_id_for_path(&nested_cwd).as_deref(),
            Some(child_id.as_str()),
            "longest prefix must win"
        );
        // `/x/foo` must not match `/x/foobar`.
        assert_eq!(
            resolve_project_id_for_path(&format!("{parent}extra")),
            None,
            "prefix must be path-boundary aware"
        );
        delete_project(&child_id);
        delete_project(&pid);
    }

    #[test]
    fn unknown_path_skips_insert() {
        db::init_for_tests();
        let out = record_compose_send_for_cwd(
            "/tmp/k2-compose-hist-unknown-path-no-project",
            "hello",
            "owner",
        )
        .expect("unknown path");
        assert!(out.is_none(), "unknown cwd must skip insert, got {out:?}");
    }

    #[test]
    fn delete_project_cascades_history() {
        db::init_for_tests();
        let pid = unique("cascade");
        let path = format!("/tmp/{pid}");
        ensure_project(&pid, &path, "cascade");
        record_compose_send(&pid, "gone-soon", "owner")
            .expect("insert")
            .expect("stored");
        assert_eq!(count_for(&pid), 1);
        delete_project(&pid);
        assert_eq!(count_for(&pid), 0, "ON DELETE CASCADE must drop history");
        assert!(
            !project_exists(&pid),
            "project row must be gone after delete"
        );
    }

    #[test]
    fn list_for_query_unknown_path_is_empty() {
        db::init_for_tests();
        let items = list_for_query("", "/tmp/k2-compose-hist-no-such-ws").expect("list");
        assert!(items.is_empty(), "unknown path must yield empty list");
        let items = list_for_query("no-such-project-id", "").expect("list by id");
        assert!(items.is_empty(), "unknown project_id must yield empty list");
    }
}
