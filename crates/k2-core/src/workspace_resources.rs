//! Durable workspace resource rows (`workspace_resources`, migration 0105).
//!
//! A resource is a file the user explicitly added from the Files tree.
//! Membership is daemon-owned (not pinned-tab scrape, not localStorage).
//! `workspace_id` is `projects.id` — never the git-worktree `workspaces.id`.

use std::path::{Component, Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension};

/// One resource row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceResource {
    pub workspace_id: String,
    pub file_path: String,
    pub added_at: i64,
}

#[derive(Debug)]
pub enum ResourceError {
    /// Canonical path is outside the Files tree root (projects.path ∪ worktree_path).
    PathEscape(String),
    /// Path is missing, not a file, or a directory.
    NotAFile(String),
    /// No such (workspace_id, file_path) row.
    NotFound,
    Db(rusqlite::Error),
}

impl std::fmt::Display for ResourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResourceError::PathEscape(p) => {
                write!(f, "path_escape: path is outside the workspace tree: {p}")
            }
            ResourceError::NotAFile(p) => {
                write!(f, "not_a_file: path is not an existing file: {p}")
            }
            ResourceError::NotFound => write!(f, "not_found: resource is not in the list"),
            ResourceError::Db(e) => write!(f, "db: {e}"),
        }
    }
}

impl From<rusqlite::Error> for ResourceError {
    fn from(e: rusqlite::Error) -> Self {
        ResourceError::Db(e)
    }
}

impl ResourceError {
    pub fn code(&self) -> &'static str {
        match self {
            ResourceError::PathEscape(_) => "path_escape",
            ResourceError::NotAFile(_) => "not_a_file",
            ResourceError::NotFound => "not_found",
            ResourceError::Db(_) => "db",
        }
    }
}

fn map_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorkspaceResource> {
    Ok(WorkspaceResource {
        workspace_id: row.get(0)?,
        file_path: row.get(1)?,
        added_at: row.get(2)?,
    })
}

/// Files tree roots for a workspace: `projects.path` plus every
/// `workspaces.worktree_path` for that `projects.id`.
pub fn tree_roots(conn: &Connection, workspace_id: &str) -> Result<Vec<PathBuf>, ResourceError> {
    let mut roots: Vec<PathBuf> = Vec::new();
    let project_path: Option<String> = conn
        .query_row(
            "SELECT path FROM projects WHERE id = ?1",
            params![workspace_id],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(p) = project_path {
        if !p.is_empty() {
            roots.push(canon_root(Path::new(&p)));
        }
    }
    let mut stmt = conn.prepare(
        "SELECT worktree_path FROM workspaces \
         WHERE project_id = ?1 AND worktree_path IS NOT NULL AND worktree_path != ''",
    )?;
    let rows = stmt.query_map(params![workspace_id], |r| r.get::<_, String>(0))?;
    for r in rows {
        let p = r?;
        roots.push(canon_root(Path::new(&p)));
    }
    Ok(roots)
}

fn canon_root(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| lexical_abs(path))
}

/// Absolute + `..` / `.` collapsed without requiring the path to exist.
fn lexical_abs(path: &Path) -> PathBuf {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("/"))
            .join(path)
    };
    let mut out = PathBuf::new();
    for c in abs.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn is_under(child: &Path, root: &Path) -> bool {
    child.starts_with(root) && child != root
}

/// Canonicalize `raw_path` and accept it only when it stays under a tree root.
pub fn confine_path(
    conn: &Connection,
    workspace_id: &str,
    raw_path: &str,
) -> Result<PathBuf, ResourceError> {
    let raw = raw_path.trim();
    if raw.is_empty() {
        return Err(ResourceError::NotAFile(raw_path.to_string()));
    }
    let path = Path::new(raw);
    let canon = std::fs::canonicalize(path).unwrap_or_else(|_| lexical_abs(path));
    if !canon.is_file() {
        return Err(ResourceError::NotAFile(canon.display().to_string()));
    }
    let roots = tree_roots(conn, workspace_id)?;
    if !roots.iter().any(|r| is_under(&canon, r)) {
        return Err(ResourceError::PathEscape(canon.display().to_string()));
    }
    Ok(canon)
}

pub fn list(conn: &Connection, workspace_id: &str) -> Result<Vec<WorkspaceResource>, ResourceError> {
    let mut stmt = conn.prepare(
        "SELECT workspace_id, file_path, added_at FROM workspace_resources \
         WHERE workspace_id = ?1 ORDER BY added_at ASC, file_path ASC",
    )?;
    let rows = stmt.query_map(params![workspace_id], map_row)?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

/// INSERT OR IGNORE. Duplicate add is success with one row.
pub fn add(
    conn: &Connection,
    workspace_id: &str,
    raw_path: &str,
) -> Result<String, ResourceError> {
    let canon = confine_path(conn, workspace_id, raw_path)?;
    let stored = canon.to_string_lossy().to_string();
    conn.execute(
        "INSERT OR IGNORE INTO workspace_resources (workspace_id, file_path, added_at) \
         VALUES (?1, ?2, unixepoch())",
        params![workspace_id, stored],
    )?;
    Ok(stored)
}

/// Insert without confinement (boot migration from layout blobs).
pub fn insert_ignore(
    conn: &Connection,
    workspace_id: &str,
    file_path: &str,
) -> Result<(), ResourceError> {
    if workspace_id.is_empty() || file_path.is_empty() {
        return Ok(());
    }
    conn.execute(
        "INSERT OR IGNORE INTO workspace_resources (workspace_id, file_path, added_at) \
         VALUES (?1, ?2, unixepoch())",
        params![workspace_id, file_path],
    )?;
    Ok(())
}

pub fn remove(
    conn: &Connection,
    workspace_id: &str,
    raw_path: &str,
) -> Result<(), ResourceError> {
    let raw = raw_path.trim();
    if raw.is_empty() {
        return Err(ResourceError::NotFound);
    }
    // Prefer the stored (canonical) path; also try the raw token so a
    // client that still holds a pre-canonical listing can remove it.
    let candidates: Vec<String> = {
        let mut v = vec![raw.to_string()];
        if let Ok(c) = std::fs::canonicalize(raw) {
            let s = c.to_string_lossy().to_string();
            if s != raw {
                v.push(s);
            }
        } else {
            let s = lexical_abs(Path::new(raw)).to_string_lossy().to_string();
            if s != raw {
                v.push(s);
            }
        }
        v
    };
    for p in &candidates {
        let n = conn.execute(
            "DELETE FROM workspace_resources WHERE workspace_id = ?1 AND file_path = ?2",
            params![workspace_id, p],
        )?;
        if n > 0 {
            return Ok(());
        }
    }
    Err(ResourceError::NotFound)
}

pub fn file_missing(file_path: &str) -> bool {
    !Path::new(file_path).is_file()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::isolated_test_connection;
    use std::fs;
    use std::io::Write;

    fn unique_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "k2-wsres-{label}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    fn touch(path: &Path) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).ok();
        }
        let mut f = fs::File::create(path).expect("create file");
        f.write_all(b"x").ok();
    }

    fn seed_project(conn: &Connection, path: &str) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
            params![id, format!("ws-{id}"), path],
        )
        .expect("insert project");
        id
    }

    #[test]
    fn add_twice_one_row() {
        let dir = unique_dir("dup");
        let file = dir.join("a.csv");
        touch(&file);
        let conn = isolated_test_connection();
        let id = seed_project(&conn, dir.to_str().unwrap());
        let p1 = add(&conn, &id, file.to_str().unwrap()).unwrap();
        let p2 = add(&conn, &id, file.to_str().unwrap()).unwrap();
        assert_eq!(p1, p2);
        let rows = list(&conn, &id).unwrap();
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].file_path, p1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn path_outside_tree_is_escape() {
        let dir = unique_dir("in");
        let sibling = dir.parent().unwrap().join(format!(
            "k2-wsres-escape-{}",
            uuid::Uuid::new_v4()
        ));
        touch(&sibling);
        let via_dotdot = dir.join("..").join(sibling.file_name().unwrap());
        let conn = isolated_test_connection();
        let id = seed_project(&conn, dir.to_str().unwrap());
        let err = add(&conn, &id, via_dotdot.to_str().unwrap()).unwrap_err();
        assert_eq!(err.code(), "path_escape", "{err}");
        assert!(list(&conn, &id).unwrap().is_empty());
        let _ = fs::remove_file(&sibling);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn worktree_file_not_under_projects_path_is_ok() {
        let main = unique_dir("main");
        let wt = unique_dir("wt");
        let file = wt.join("note.csv");
        touch(&file);
        let conn = isolated_test_connection();
        let id = seed_project(&conn, main.to_str().unwrap());
        conn.execute(
            "INSERT INTO workspaces (id, project_id, name, worktree_path) VALUES (?1, ?2, 'wt', ?3)",
            params![uuid::Uuid::new_v4().to_string(), id, wt.to_str().unwrap()],
        )
        .expect("insert worktree");
        let stored = add(&conn, &id, file.to_str().unwrap()).expect("add worktree file");
        let rows = list(&conn, &id).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].file_path, stored);
        let _ = fs::remove_dir_all(&main);
        let _ = fs::remove_dir_all(&wt);
    }

    #[test]
    fn remove_missing_is_not_found() {
        let dir = unique_dir("rm");
        let conn = isolated_test_connection();
        let id = seed_project(&conn, dir.to_str().unwrap());
        let err = remove(&conn, &id, "/no/such/file.txt").unwrap_err();
        assert_eq!(err.code(), "not_found");
        let _ = fs::remove_dir_all(&dir);
    }
}
