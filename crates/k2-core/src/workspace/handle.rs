//! Workspace handle (street address) vs Agent Name (display).
//!
//! SSOT: `.k2/prds/prd-workspace-display-name-and-handle-v1.md`.
//! `projects.handle` + AGENT.md `name:` are the address. `projects.name`
//! + AGENT.md `display_name:` are wallpaper. Copy pretty first, then slug.

use std::fs;
use std::path::Path;

use rusqlite::{params, Connection};

use crate::workspace::agent_identity::{
    backup_sibling_legacy_persona, parse_frontmatter, persona_md_in, workspace_agent_md_path,
    workspace_agent_path,
};
use crate::workspace::display::{invalidate_agent_display_name_cache, rewrite_frontmatter_field};
use crate::workspace_session_handles::{
    is_uuid_shape, normalize_address_token, slugify_address_token,
};

/// Outcome of resolving a local workspace token (D19 / §9.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceTokenResolve {
    Found { path: String },
    /// Display-name (or other) collision — fail-closed with both handles.
    Ambiguous { handles: Vec<String> },
    Miss,
}

/// Allocate a host-unique handle from `seed` (`slug`, then `slug-2`, …).
/// `exclude_project_id` is the row being updated (self does not collide).
pub fn allocate_unique_handle(
    conn: &Connection,
    seed: &str,
    exclude_project_id: Option<&str>,
) -> Result<String, String> {
    let base = match slugify_address_token(seed) {
        Ok(s) => s,
        Err(_) => {
            let fallback = Path::new(seed)
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            slugify_address_token(&fallback).unwrap_or_else(|_| "agent".to_string())
        }
    };
    if !handle_taken(conn, &base, exclude_project_id) {
        return Ok(base);
    }
    let mut n = 2u32;
    loop {
        let candidate = format!("{base}-{n}");
        if !handle_taken(conn, &candidate, exclude_project_id) {
            crate::log_debug!(
                "[handle] collision on '{base}'; minted '{candidate}'"
            );
            return Ok(candidate);
        }
        n = n.checked_add(1).ok_or_else(|| {
            format!("handle '{base}' exhausted numeric suffixes")
        })?;
        if n > 10_000 {
            return Err(format!("handle '{base}' has too many collisions"));
        }
    }
}

fn handle_taken(conn: &Connection, candidate: &str, exclude_project_id: Option<&str>) -> bool {
    let self_id = exclude_project_id.unwrap_or("");
    let hit: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM projects \
             WHERE handle = ?1 COLLATE NOCASE \
               AND (?2 = '' OR id != ?2)",
            params![candidate, self_id],
            |r| r.get(0),
        )
        .unwrap_or(0);
    if hit > 0 {
        return true;
    }
    let alias_hit: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM project_handle_aliases \
             WHERE alias = ?1 COLLATE NOCASE \
               AND (?2 = '' OR project_id != ?2)",
            params![candidate, self_id],
            |r| r.get(0),
        )
        .unwrap_or(0);
    alias_hit > 0
}

/// Name the workspace that already owns `candidate` (handle or alias).
pub fn handle_collision_owner(
    conn: &Connection,
    candidate: &str,
    exclude_project_id: Option<&str>,
) -> Option<String> {
    let self_id = exclude_project_id.unwrap_or("");
    if let Ok(name) = conn.query_row(
        "SELECT COALESCE(NULLIF(TRIM(name), ''), path) FROM projects \
         WHERE handle = ?1 COLLATE NOCASE AND (?2 = '' OR id != ?2) LIMIT 1",
        params![candidate, self_id],
        |r| r.get::<_, String>(0),
    ) {
        return Some(name);
    }
    conn.query_row(
        "SELECT COALESCE(NULLIF(TRIM(p.name), ''), p.path) \
         FROM project_handle_aliases a \
         JOIN projects p ON p.id = a.project_id \
         WHERE a.alias = ?1 COLLATE NOCASE AND (?2 = '' OR a.project_id != ?2) LIMIT 1",
        params![candidate, self_id],
        |r| r.get::<_, String>(0),
    )
    .ok()
}

/// INSERT OR IGNORE an alias. Never fails the caller on collision (D13).
pub fn insert_alias_or_ignore(conn: &Connection, project_id: &str, alias: &str) {
    let alias = alias.trim();
    if alias.is_empty() {
        return;
    }
    if let Ok(handle) = project_handle(conn, project_id) {
        if handle.eq_ignore_ascii_case(alias) {
            return;
        }
    }
    match conn.execute(
        "INSERT OR IGNORE INTO project_handle_aliases (project_id, alias) VALUES (?1, ?2)",
        params![project_id, alias],
    ) {
        Ok(0) => crate::log_debug!(
            "[handle] alias '{alias}' skipped (already claimed) for {project_id}"
        ),
        Ok(_) => {}
        Err(e) => crate::log_debug!("[handle] alias insert '{alias}' failed: {e}"),
    }
}

pub fn aliases_for(conn: &Connection, project_id: &str) -> Vec<String> {
    let mut stmt = match conn.prepare(
        "SELECT alias FROM project_handle_aliases WHERE project_id = ?1 ORDER BY alias",
    ) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    stmt.query_map(params![project_id], |r| r.get::<_, String>(0))
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
}

pub fn project_handle(conn: &Connection, project_id: &str) -> Result<String, String> {
    crate::workspace_session_handles::workspace_address_name(conn, project_id)
}

pub fn project_handle_for_path(conn: &Connection, path: &str) -> Option<String> {
    let id: String = conn
        .query_row(
            "SELECT id FROM projects WHERE path = ?1",
            params![path],
            |r| r.get(0),
        )
        .ok()?;
    project_handle(conn, &id).ok()
}

/// Mint a handle for a just-inserted (or about-to-insert) project.
pub fn mint_handle_for_create(conn: &Connection, display_or_folder: &str) -> String {
    allocate_unique_handle(conn, display_or_folder, None).unwrap_or_else(|_| "agent".to_string())
}

/// D12/D11 writer: set handle, rewrite AGENT.md `name:`, alias the previous.
/// Does **not** rewrite display / `projects.name`.
pub fn set_workspace_handle(project_path: &str, requested: &str) -> Result<String, String> {
    let slug = slugify_address_token(requested)?;
    let db = crate::db::shared();
    let conn = db.lock();
    let (id, old_handle): (String, Option<String>) = conn
        .query_row(
            "SELECT id, handle FROM projects WHERE path = ?1",
            params![project_path],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|_| format!("workspace not found: {project_path}"))?;
    if let Some(other) = handle_collision_owner(&conn, &slug, Some(&id)) {
        return Err(format!(
            "Handle '{slug}' is already used by workspace '{other}'."
        ));
    }
    conn.execute(
        "UPDATE projects SET handle = ?1 WHERE id = ?2",
        params![&slug, &id],
    )
    .map_err(|e| format!("failed to update projects.handle: {e}"))?;
    if let Some(old) = old_handle {
        let old = old.trim();
        if !old.is_empty() && !old.eq_ignore_ascii_case(&slug) {
            insert_alias_or_ignore(&conn, &id, old);
        }
    }
    drop(conn);
    rewrite_agent_md_name(project_path, &slug)?;
    invalidate_agent_display_name_cache(project_path);
    Ok(slug)
}

fn rewrite_agent_md_name(project_path: &str, handle: &str) -> Result<(), String> {
    let dir = workspace_agent_path(project_path);
    let live = persona_md_in(&dir);
    if !live.exists() {
        return Ok(());
    }
    let content = fs::read_to_string(&live)
        .map_err(|e| format!("Cannot read persona at {}: {}", live.display(), e))?;
    let updated = rewrite_frontmatter_field(&content, "name", handle);
    let dest = workspace_agent_md_path(project_path);
    crate::workspace::work_item::atomic_write(&dest, &updated)?;
    backup_sibling_legacy_persona(&dir);
    Ok(())
}

/// §8 boot backfill. Idempotent. Never fails daemon boot (per-row errors
/// are logged). Uses the passed connection — does **not** call
/// `db::shared()` so it is safe during `init_database` before SHARED is set.
pub fn backfill_workspace_handles(conn: &Connection) {
    let rows: Vec<(i64, String, String, String, Option<String>)> = {
        let mut stmt = match conn.prepare(
            "SELECT rowid, id, name, path, handle FROM projects ORDER BY rowid",
        ) {
            Ok(s) => s,
            Err(e) => {
                crate::log_debug!("[handle] 0103 backfill prepare failed: {e}");
                return;
            }
        };
        let mapped = match stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<String>>(4)?,
            ))
        }) {
            Ok(rows) => rows.flatten().collect(),
            Err(e) => {
                crate::log_debug!("[handle] 0103 backfill scan failed: {e}");
                return;
            }
        };
        mapped
    };

    for (_rowid, id, name, path, existing_handle) in rows {
        if let Err(e) = backfill_one(conn, &id, &name, &path, existing_handle.as_deref()) {
            crate::log_debug!("[handle] 0103 backfill skipped {id} ({path}): {e}");
        }
    }

    rewrite_remote_connection_agents(conn);
}

fn backfill_one(
    conn: &Connection,
    id: &str,
    projects_name: &str,
    path: &str,
    existing_handle: Option<&str>,
) -> Result<(), String> {
    let dir = workspace_agent_path(path);
    let md_path = persona_md_in(&dir);
    let md_content = fs::read_to_string(&md_path).ok();
    let fm = md_content
        .as_deref()
        .map(parse_frontmatter)
        .unwrap_or_default();
    let prev_name = fm
        .get("name")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let pretty = live_pretty_name(path, projects_name, &fm);

    if md_content.is_some() {
        let display_empty = fm
            .get("display_name")
            .map(|s| s.trim().is_empty())
            .unwrap_or(true);
        if display_empty {
            let content = md_content.as_deref().unwrap_or("---\n---\n\n");
            let updated = rewrite_frontmatter_field(content, "display_name", &pretty);
            let dest = workspace_agent_md_path(path);
            let _ = crate::workspace::work_item::atomic_write(&dest, &updated);
            backup_sibling_legacy_persona(&dir);
        }
    }

    let name_needs_pretty = {
        let t = projects_name.trim();
        t.is_empty() || is_uuid_shape(t) || t != pretty
    };
    if name_needs_pretty {
        let _ = conn.execute(
            "UPDATE projects SET name = ?1 WHERE id = ?2",
            params![&pretty, id],
        );
    }

    let handle = match existing_handle.map(str::trim).filter(|s| !s.is_empty()) {
        Some(h) => h.to_string(),
        None => allocate_unique_handle(conn, &pretty, Some(id))?,
    };
    let _ = conn.execute(
        "UPDATE projects SET handle = ?1 WHERE id = ?2",
        params![&handle, id],
    );

    if let Some(content) = fs::read_to_string(&md_path).ok() {
        let updated = rewrite_frontmatter_field(&content, "name", &handle);
        let _ = crate::workspace::work_item::atomic_write(&md_path, &updated);
    }

    let pretty_lc = pretty.to_lowercase();
    if pretty_lc != handle {
        insert_alias_or_ignore(conn, id, &pretty_lc);
    }
    if let Some(base) = Path::new(path)
        .file_name()
        .map(|s| s.to_string_lossy().to_lowercase())
        .filter(|s| !s.is_empty() && s != &handle)
    {
        insert_alias_or_ignore(conn, id, &base);
    }
    if let Some(prev) = prev_name {
        let prev_lc = prev.to_lowercase();
        if prev_lc != handle && prev_lc != pretty_lc {
            insert_alias_or_ignore(conn, id, &prev_lc);
        }
    }

    invalidate_agent_display_name_cache(path);
    Ok(())
}

fn live_pretty_name(
    path: &str,
    projects_name: &str,
    fm: &std::collections::HashMap<String, String>,
) -> String {
    if let Some(d) = fm.get("display_name").map(|s| s.trim()).filter(|s| !s.is_empty()) {
        return d.to_string();
    }
    if let Some(n) = fm.get("name").map(|s| s.trim()).filter(|s| !s.is_empty()) {
        return n.to_string();
    }
    let name = projects_name.trim();
    if !name.is_empty() && !is_uuid_shape(name) {
        return name.to_string();
    }
    if let Some(base) = Path::new(path)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .filter(|s| !s.trim().is_empty())
    {
        return base;
    }
    "agent".to_string()
}

fn rewrite_remote_connection_agents(conn: &Connection) {
    let rows: Vec<(String, String, String, String, String)> = {
        let mut stmt = match conn.prepare(
            "SELECT id, source_project_id, remote_addr, host, agent \
             FROM workspace_remote_connections",
        ) {
            Ok(s) => s,
            Err(_) => return,
        };
        let mapped = match stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
            ))
        }) {
            Ok(rows) => rows.flatten().collect(),
            Err(_) => return,
        };
        mapped
    };
    for (id, source, remote_addr, host, agent) in rows {
        let Ok(new_agent) = slugify_address_token(&agent) else {
            continue;
        };
        if new_agent == agent {
            continue;
        }
        let new_addr = if remote_addr.contains("::") {
            format!("{new_agent}::{host}")
        } else if remote_addr.contains('@') {
            format!("{new_agent}@{host}")
        } else {
            format!("{new_agent}::{host}")
        };
        match conn.execute(
            "UPDATE workspace_remote_connections \
             SET agent = ?1, remote_addr = ?2 WHERE id = ?3",
            params![&new_agent, &new_addr, &id],
        ) {
            Ok(_) => {}
            Err(e) if e.to_string().contains("UNIQUE") => {
                let _ = conn.execute(
                    "DELETE FROM workspace_remote_connections WHERE id = ?1",
                    params![&id],
                );
                crate::log_debug!(
                    "[handle] dropped duplicate remote row {id} ({source} {agent} → {new_agent})"
                );
            }
            Err(e) => crate::log_debug!("[handle] remote rewrite {id} failed: {e}"),
        }
    }
}

/// §9.5 local resolve: path → UUID → handle → alias → name → unique basename.
/// Name collisions are fail-closed (AMBIG), not first-rowid.
pub fn resolve_workspace_token(conn: &Connection, token: &str) -> WorkspaceTokenResolve {
    let token = token.trim();
    if token.is_empty() {
        return WorkspaceTokenResolve::Miss;
    }

    if token.starts_with('/') {
        return match conn.query_row(
            "SELECT path FROM projects WHERE path = ?1",
            params![token],
            |r| r.get::<_, String>(0),
        ) {
            Ok(path) => WorkspaceTokenResolve::Found { path },
            Err(_) => WorkspaceTokenResolve::Miss,
        };
    }

    if is_uuid_shape(token) {
        if let Ok(path) = conn.query_row(
            "SELECT path FROM projects WHERE id = ?1",
            params![token],
            |r| r.get::<_, String>(0),
        ) {
            return WorkspaceTokenResolve::Found { path };
        }
    }

    // Handle exact / NOCASE (unique index → 0 or 1).
    let handles: Vec<String> = query_paths(
        conn,
        "SELECT path FROM projects WHERE handle = ?1 COLLATE NOCASE AND handle IS NOT NULL AND TRIM(handle) != ''",
        token,
    );
    match handles.len() {
        1 => return WorkspaceTokenResolve::Found {
            path: handles.into_iter().next().unwrap(),
        },
        n if n > 1 => {
            return WorkspaceTokenResolve::Ambiguous {
                handles: handles_for_paths(conn, &handles),
            };
        }
        _ => {}
    }

    // Alias (unique index → 0 or 1).
    if let Ok(path) = conn.query_row(
        "SELECT p.path FROM project_handle_aliases a \
         JOIN projects p ON p.id = a.project_id \
         WHERE a.alias = ?1 COLLATE NOCASE",
        params![token],
        |r| r.get::<_, String>(0),
    ) {
        return WorkspaceTokenResolve::Found { path };
    }

    // Display name exact, then NOCASE — fail-closed on collision.
    let exact = query_paths(conn, "SELECT path FROM projects WHERE name = ?1", token);
    match exact.len() {
        1 => return WorkspaceTokenResolve::Found {
            path: exact.into_iter().next().unwrap(),
        },
        n if n > 1 => {
            return WorkspaceTokenResolve::Ambiguous {
                handles: handles_for_paths(conn, &exact),
            };
        }
        _ => {}
    }
    let nocase = query_paths(
        conn,
        "SELECT path FROM projects WHERE name = ?1 COLLATE NOCASE",
        token,
    );
    match nocase.len() {
        1 => {
            return WorkspaceTokenResolve::Found {
                path: nocase.into_iter().next().unwrap(),
            };
        }
        n if n > 1 => {
            return WorkspaceTokenResolve::Ambiguous {
                handles: handles_for_paths(conn, &nocase),
            };
        }
        _ => {}
    }

    // Unique folder basename.
    let mut base_matches: Vec<String> = Vec::new();
    if let Ok(mut stmt) = conn.prepare("SELECT path FROM projects") {
        if let Ok(rows) = stmt.query_map([], |r| r.get::<_, String>(0)) {
            for path in rows.flatten() {
                let matches_base = Path::new(&path)
                    .file_name()
                    .map(|b| b.to_string_lossy().eq_ignore_ascii_case(token))
                    .unwrap_or(false);
                if matches_base {
                    base_matches.push(path);
                }
            }
        }
    }
    match base_matches.len() {
        1 => WorkspaceTokenResolve::Found {
            path: base_matches.into_iter().next().unwrap(),
        },
        n if n > 1 => WorkspaceTokenResolve::Ambiguous {
            handles: handles_for_paths(conn, &base_matches),
        },
        _ => WorkspaceTokenResolve::Miss,
    }
}

fn query_paths(conn: &Connection, sql: &str, token: &str) -> Vec<String> {
    let mut stmt = match conn.prepare(sql) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    stmt.query_map(params![token], |r| r.get::<_, String>(0))
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
}

fn handles_for_paths(conn: &Connection, paths: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for path in paths {
        if let Some(h) = project_handle_for_path(conn, path) {
            if !h.is_empty() {
                out.push(h);
                continue;
            }
        }
        out.push(path.clone());
    }
    out.sort();
    out.dedup();
    out
}

/// Roster / CLI matcher: want vs handle + aliases + optional workspace_name.
pub fn roster_entry_matches(want: &str, handle: &str, aliases: &[String], workspace_name: &str) -> bool {
    let want_n = normalize_address_token(want);
    if want_n == normalize_address_token(handle) {
        return true;
    }
    if aliases
        .iter()
        .any(|a| want_n == normalize_address_token(a))
    {
        return true;
    }
    !workspace_name.trim().is_empty() && want_n == normalize_address_token(workspace_name)
}

/// Lazy-heal stored remote rows whose agent already matches a roster
/// handle or alias (D10). Never attaches leftover `sales` to "the only
/// agent on that host" (D20).
pub fn heal_remote_connections_from_roster(
    conn: &Connection,
    roster: &[(String, Vec<String>)],
) {
    let rows: Vec<(String, String, String, String)> = {
        let mut stmt = match conn.prepare(
            "SELECT id, remote_addr, host, agent FROM workspace_remote_connections",
        ) {
            Ok(s) => s,
            Err(_) => return,
        };
        let mapped = match stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        }) {
            Ok(rows) => rows.flatten().collect(),
            Err(_) => return,
        };
        mapped
    };
    for (id, remote_addr, host, agent) in rows {
        let mut matched: Option<&str> = None;
        for (handle, aliases) in roster {
            if roster_entry_matches(&agent, handle, aliases, "") {
                matched = Some(handle.as_str());
                break;
            }
        }
        let Some(canonical) = matched else {
            continue;
        };
        if agent == canonical {
            continue;
        }
        let new_addr = if remote_addr.contains("::") {
            format!("{canonical}::{host}")
        } else if remote_addr.contains('@') {
            format!("{canonical}@{host}")
        } else {
            format!("{canonical}::{host}")
        };
        match conn.execute(
            "UPDATE workspace_remote_connections SET agent = ?1, remote_addr = ?2 WHERE id = ?3",
            params![canonical, new_addr, id],
        ) {
            Ok(_) => {}
            Err(e) if e.to_string().contains("UNIQUE") => {
                let _ = conn.execute(
                    "DELETE FROM workspace_remote_connections WHERE id = ?1",
                    params![id],
                );
            }
            Err(e) => crate::log_debug!("[handle] lazy heal {id} failed: {e}"),
        }
    }
}

/// Slugs this workspace should accept as `/v1/w/<slug>` / wiki grants.
pub fn slug_candidates_for_path(conn: &Connection, path: &str) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(base) = Path::new(path)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty() && !s.contains('/'))
    {
        out.push(base);
    }
    let row: Option<(Option<String>, String, String)> = conn
        .query_row(
            "SELECT handle, name, id FROM projects WHERE path = ?1 LIMIT 1",
            params![path],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .ok();
    if let Some((handle, name, id)) = row {
        if let Some(h) = handle {
            let t = h.trim();
            if !t.is_empty() && !out.iter().any(|s| s == t) {
                out.push(t.to_string());
            }
        }
        let t = name.trim();
        if !t.is_empty() && !t.contains('/') && !out.iter().any(|s| s == t) {
            out.push(t.to_string());
        }
        for alias in aliases_for(conn, &id) {
            let t = alias.trim();
            if !t.is_empty() && !t.contains('/') && !out.iter().any(|s| s == t) {
                out.push(t.to_string());
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::workspace::display::{agent_display_name, set_agent_display_name};

    fn unique_dir(label: &str) -> (String, String) {
        let id = uuid::Uuid::new_v4().to_string();
        let dir = std::env::temp_dir().join(format!(
            "k2-handle-{label}-{}-{id}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("mkdir");
        (id, dir.to_string_lossy().into_owned())
    }

    fn unique_pretty(id: &str, base: &str) -> String {
        format!("{base} {id}", id = &id[..8])
    }

    fn insert_project(id: &str, name: &str, path: &str) {
        let dbh = db::shared();
        let conn = dbh.lock();
        conn.execute(
            "INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
            params![id, name, path],
        )
        .expect("insert project");
    }

    fn write_agent_md(path: &str, display: &str, name: &str) {
        let dir = workspace_agent_md_path(path);
        if let Some(parent) = dir.parent() {
            std::fs::create_dir_all(parent).expect("mkdir agent");
        }
        std::fs::write(
            &dir,
            format!("---\nname: {name}\ndisplay_name: {display}\ntype: custom\n---\n# persona\n"),
        )
        .expect("write AGENT.md");
    }

    fn handle_of(id: &str) -> String {
        let dbh = db::shared();
        let conn = dbh.lock();
        conn.query_row(
            "SELECT handle FROM projects WHERE id = ?1",
            params![id],
            |r| r.get::<_, Option<String>>(0),
        )
        .expect("handle col")
        .expect("handle set")
    }

    fn aliases_of(id: &str) -> Vec<String> {
        let dbh = db::shared();
        let conn = dbh.lock();
        aliases_for(&conn, id)
    }

    #[test]
    fn backfill_sales_team_copies_pretty_then_slugs() {
        crate::db::init_for_tests();
        let (id, path) = unique_dir("sales");
        let pretty = unique_pretty(&id, "Sales Team");
        insert_project(&id, &pretty, &path);
        write_agent_md(&path, &pretty, &pretty);

        {
            let dbh = db::shared();
            let conn = dbh.lock();
            backfill_workspace_handles(&conn);
        }

        assert_eq!(agent_display_name(&path), pretty);
        let handle = handle_of(&id);
        let expected_slug = slugify_address_token(&pretty).expect("slug");
        assert_eq!(handle, expected_slug);
        let md = std::fs::read_to_string(workspace_agent_md_path(&path)).expect("md");
        assert!(
            md.lines().any(|l| l.trim() == format!("name: {handle}")),
            "name: must be handle; got:\n{md}"
        );
        assert!(
            md.lines().any(|l| l.trim() == format!("display_name: {pretty}")),
            "display stays pretty; got:\n{md}"
        );
        let aliases = aliases_of(&id);
        let pretty_lc = pretty.to_lowercase();
        assert!(
            aliases.iter().any(|a| a == &pretty_lc),
            "pre-slug lowercase alias missing: {aliases:?}"
        );
        let base = Path::new(&path)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_lowercase();
        assert!(
            aliases.iter().any(|a| a == &base),
            "basename alias missing: {aliases:?}"
        );
        std::fs::remove_dir_all(&path).ok();
    }

    #[test]
    fn backfill_copies_missing_display_name_then_slugs() {
        crate::db::init_for_tests();
        let (id, path) = unique_dir("nodisp");
        let pretty = unique_pretty(&id, "QA Bot");
        insert_project(&id, &pretty, &path);
        let dir = workspace_agent_md_path(&path);
        std::fs::create_dir_all(dir.parent().unwrap()).unwrap();
        std::fs::write(&dir, format!("---\nname: {pretty}\ntype: custom\n---\n")).unwrap();

        {
            let dbh = db::shared();
            let conn = dbh.lock();
            backfill_workspace_handles(&conn);
        }

        assert_eq!(agent_display_name(&path), pretty);
        assert_eq!(handle_of(&id), slugify_address_token(&pretty).expect("slug"));
        let md = std::fs::read_to_string(&dir).expect("md");
        assert!(
            md.lines().any(|l| l.trim() == format!("display_name: {pretty}")),
            "must copy into display_name; got:\n{md}"
        );
        std::fs::remove_dir_all(&path).ok();
    }

    #[test]
    fn backfill_already_slugged_does_not_suffix() {
        crate::db::init_for_tests();
        let (id, path) = unique_dir("slugged");
        let token = format!("slugged{}", &id[..8]);
        insert_project(&id, &token, &path);
        write_agent_md(&path, &token, &token);
        {
            let dbh = db::shared();
            let conn = dbh.lock();
            backfill_workspace_handles(&conn);
        }
        assert_eq!(handle_of(&id), token);
        std::fs::remove_dir_all(&path).ok();
    }

    #[test]
    fn backfill_collision_suffixes_second() {
        crate::db::init_for_tests();
        let (id_a, path_a) = unique_dir("col-a");
        let (id_b, path_b) = unique_dir("col-b");
        let pretty = unique_pretty(&id_a, "Collide Team");
        let slug = slugify_address_token(&pretty).expect("slug");
        insert_project(&id_a, &pretty, &path_a);
        insert_project(&id_b, &slug, &path_b);
        write_agent_md(&path_a, &pretty, &pretty);
        write_agent_md(&path_b, &slug, &slug);
        {
            let dbh = db::shared();
            let conn = dbh.lock();
            backfill_workspace_handles(&conn);
        }
        let ha = handle_of(&id_a);
        let hb = handle_of(&id_b);
        assert_ne!(ha, hb, "handles must differ");
        assert!(
            ha == slug || hb == slug,
            "first slug kept: {ha} / {hb}"
        );
        assert!(
            ha == format!("{slug}-2") || hb == format!("{slug}-2"),
            "second gets -2: {ha} / {hb}"
        );
        std::fs::remove_dir_all(&path_a).ok();
        std::fs::remove_dir_all(&path_b).ok();
    }

    #[test]
    fn backfill_two_cortana_folders_skips_second_basename_alias() {
        crate::db::init_for_tests();
        let suffix = uuid::Uuid::new_v4();
        let dir_a = std::env::temp_dir().join(format!("Cortana-a-{}", suffix));
        let dir_b = std::env::temp_dir().join(format!("Cortana-b-{}", suffix));
        // Same basename via nested .../Cortana
        let path_a = dir_a.join("Cortana");
        let path_b = dir_b.join("Cortana");
        std::fs::create_dir_all(&path_a).unwrap();
        std::fs::create_dir_all(&path_b).unwrap();
        let id_a = uuid::Uuid::new_v4().to_string();
        let id_b = uuid::Uuid::new_v4().to_string();
        insert_project(&id_a, "Alpha", path_a.to_str().unwrap());
        insert_project(&id_b, "Beta", path_b.to_str().unwrap());
        write_agent_md(path_a.to_str().unwrap(), "Alpha", "Alpha");
        write_agent_md(path_b.to_str().unwrap(), "Beta", "Beta");
        {
            let dbh = db::shared();
            let conn = dbh.lock();
            backfill_workspace_handles(&conn);
        }
        // Boot must succeed; exactly one workspace owns the cortana alias.
        let dbh = db::shared();
        let conn = dbh.lock();
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM project_handle_aliases WHERE alias = 'cortana' COLLATE NOCASE",
                [],
                |r| r.get(0),
            )
            .expect("count");
        assert_eq!(n, 1, "second Cortana basename alias must be OR IGNORE skipped");
        std::fs::remove_dir_all(&dir_a).ok();
        std::fs::remove_dir_all(&dir_b).ok();
    }

    #[test]
    fn set_display_does_not_change_handle_or_name_colon() {
        crate::db::init_for_tests();
        let (id, path) = unique_dir("disp");
        let pretty = unique_pretty(&id, "Sales Team");
        insert_project(&id, &pretty, &path);
        write_agent_md(&path, &pretty, &pretty);
        {
            let dbh = db::shared();
            let conn = dbh.lock();
            backfill_workspace_handles(&conn);
        }
        let before = handle_of(&id);
        assert_eq!(before, slugify_address_token(&pretty).expect("slug"));
        set_agent_display_name(&path, "Revenue Desk").expect("display rename");
        assert_eq!(agent_display_name(&path), "Revenue Desk");
        assert_eq!(handle_of(&id), before, "handle must stay");
        let md = std::fs::read_to_string(workspace_agent_md_path(&path)).expect("md");
        assert!(
            md.lines().any(|l| l.trim() == format!("name: {before}")),
            "name: must stay handle; got:\n{md}"
        );
        std::fs::remove_dir_all(&path).ok();
    }

    #[test]
    fn set_handle_aliases_previous() {
        crate::db::init_for_tests();
        let (id, path) = unique_dir("set-h");
        let pretty = unique_pretty(&id, "Sales Team");
        insert_project(&id, &pretty, &path);
        write_agent_md(&path, &pretty, &pretty);
        {
            let dbh = db::shared();
            let conn = dbh.lock();
            backfill_workspace_handles(&conn);
        }
        let old = handle_of(&id);
        let next = format!("revenue-{}", &id[..8]);
        let got = set_workspace_handle(&path, &next).expect("set-handle");
        assert_eq!(got, next);
        assert_eq!(handle_of(&id), next);
        let aliases = aliases_of(&id);
        assert!(
            aliases.iter().any(|a| a == &old),
            "previous handle aliased: {aliases:?}"
        );
        std::fs::remove_dir_all(&path).ok();
    }

    #[test]
    fn set_handle_collision_names_other_workspace() {
        crate::db::init_for_tests();
        let (id_a, path_a) = unique_dir("uniq-a");
        let (id_b, path_b) = unique_dir("uniq-b");
        insert_project(&id_a, "Alpha", &path_a);
        insert_project(&id_b, "Beta", &path_b);
        write_agent_md(&path_a, "Alpha", "Alpha");
        write_agent_md(&path_b, "Beta", "Beta");
        {
            let dbh = db::shared();
            let conn = dbh.lock();
            backfill_workspace_handles(&conn);
        }
        let token = format!("uniq-handle-{}", &id_a[..8]);
        set_workspace_handle(&path_a, &token).expect("first claim");
        let err = set_workspace_handle(&path_b, &token).expect_err("collision");
        assert!(
            err.to_ascii_lowercase().contains("already used"),
            "expected uniqueness error, got: {err}"
        );
        std::fs::remove_dir_all(&path_a).ok();
        std::fs::remove_dir_all(&path_b).ok();
    }

    #[test]
    fn resolve_order_handle_alias_name_basename() {
        crate::db::init_for_tests();
        let (id, path) = unique_dir("resolve");
        let pretty = unique_pretty(&id, "Sales Team");
        insert_project(&id, &pretty, &path);
        write_agent_md(&path, &pretty, &pretty);
        {
            let dbh = db::shared();
            let conn = dbh.lock();
            backfill_workspace_handles(&conn);
            let handle = project_handle(&conn, &id).expect("handle");
            match resolve_workspace_token(&conn, &handle) {
                WorkspaceTokenResolve::Found { path: p } => assert_eq!(p, path),
                other => panic!("handle miss: {other:?}"),
            }
            match resolve_workspace_token(&conn, &pretty) {
                WorkspaceTokenResolve::Found { path: p } => assert_eq!(p, path),
                other => panic!("name miss: {other:?}"),
            }
            match resolve_workspace_token(&conn, &id) {
                WorkspaceTokenResolve::Found { path: p } => assert_eq!(p, path),
                other => panic!("uuid miss: {other:?}"),
            }
            match resolve_workspace_token(&conn, &path) {
                WorkspaceTokenResolve::Found { path: p } => assert_eq!(p, path),
                other => panic!("path miss: {other:?}"),
            }
        }
        std::fs::remove_dir_all(&path).ok();
    }

    #[test]
    fn resolve_ambiguous_display_is_fail_closed() {
        crate::db::init_for_tests();
        let (id_a, path_a) = unique_dir("ambig-a");
        let (id_b, path_b) = unique_dir("ambig-b");
        let pretty = unique_pretty(&id_a, "Ambig Sales");
        insert_project(&id_a, &pretty, &path_a);
        insert_project(&id_b, &pretty, &path_b);
        write_agent_md(&path_a, &pretty, "Alpha");
        write_agent_md(&path_b, &pretty, "Beta");
        {
            let dbh = db::shared();
            let conn = dbh.lock();
            backfill_workspace_handles(&conn);
            let collide = format!("Shared Display {}", &id_a[..8]);
            conn.execute(
                "UPDATE projects SET name = ?1 WHERE id IN (?2, ?3)",
                params![&collide, &id_a, &id_b],
            )
            .unwrap();
            match resolve_workspace_token(&conn, &collide) {
                WorkspaceTokenResolve::Ambiguous { handles } => {
                    assert_eq!(handles.len(), 2, "both handles: {handles:?}");
                }
                other => panic!("expected AMBIG, got {other:?}"),
            }
        }
        std::fs::remove_dir_all(&path_a).ok();
        std::fs::remove_dir_all(&path_b).ok();
    }

    #[test]
    fn lazy_heal_rewrites_matching_row_leaves_d20() {
        crate::db::init_for_tests();
        let (id, path) = unique_dir("heal");
        insert_project(&id, "Sales Team", &path);
        {
            let dbh = db::shared();
            let conn = dbh.lock();
            crate::db::schema::WorkspaceRemoteConnection::create(
                &conn,
                "heal-match",
                &id,
                "sales team::peer.k2.dev",
                "peer.k2.dev",
                "sales team",
                None,
            )
            .unwrap();
            crate::db::schema::WorkspaceRemoteConnection::create(
                &conn,
                "heal-d20",
                &id,
                "sales::peer.k2.dev",
                "peer.k2.dev",
                "sales",
                None,
            )
            .unwrap();
            heal_remote_connections_from_roster(
                &conn,
                &[("sales-team".into(), vec!["sales team".into()])],
            );
            let rows = crate::db::schema::WorkspaceRemoteConnection::list_for_source(&conn, &id)
                .unwrap();
            let match_row = rows.iter().find(|r| r.id == "heal-match").expect("match");
            assert_eq!(match_row.agent, "sales-team");
            let leftover = rows.iter().find(|r| r.id == "heal-d20").expect("d20");
            assert_eq!(leftover.agent, "sales", "D20 leftover must stay");
        }
        std::fs::remove_dir_all(&path).ok();
    }

    #[test]
    fn create_path_mints_handle() {
        crate::db::init_for_tests();
        let (id, path) = unique_dir("create");
        {
            let dbh = db::shared();
            let conn = dbh.lock();
            let pretty = unique_pretty(&id, "Create Team");
            crate::db::schema::Project::create(
                &conn, &id, &pretty, &path, "#000", 0, 0, None, None,
            )
            .expect("create");
            let h: String = conn
                .query_row(
                    "SELECT handle FROM projects WHERE id = ?1",
                    params![id],
                    |r| r.get::<_, Option<String>>(0),
                )
                .expect("row")
                .expect("handle minted");
            assert_eq!(h, slugify_address_token(&pretty).expect("slug"));
        }
        std::fs::remove_dir_all(&path).ok();
    }
}
