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

// ── One loose federated-name matcher (CA2) ─────────────────────────────
//
// Three folds of a typed agent name are live today: the `cli/k2` roster
// lookup (`norm`: Unicode letters kept, `.` dropped), the daemon tray
// resolver (non-alphanumeric runs → `-`, so `.` → `-`), and
// `normalize_address_token` (ASCII address token). A sender that resolved
// a name with any of them puts the name as typed into the signed `to`.
// The receiver must accept it whenever ANY fold of the typed name equals
// ANY fold of one of the workspace's own names, so no message an older
// sender addressed correctly is refused.

/// The daemon tray resolver's fold: trim, ASCII-lowercase, `_` → `-`, each
/// run of non-ASCII-alphanumerics → one `-`, outer `-` trimmed.
pub fn dash_fold_name(s: &str) -> String {
    let t = s.trim().to_ascii_lowercase().replace('_', "-");
    let mut out = String::new();
    let mut dash = false;
    for c in t.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
            dash = false;
        } else if !dash {
            out.push('-');
            dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

/// The `cli/k2` remote-msg roster fold (`slugify` + `norm` in the embedded
/// Python): raw lowercase when the name has a path/control character;
/// otherwise whitespace → `-`, keep letters/digits/`-`, collapse `--`.
/// Falls back to the raw lowercase form when the slug is empty.
pub fn cli_fold_name(s: &str) -> String {
    let raw = s.trim().to_lowercase();
    if s.chars().any(|c| matches!(c, '/' | ':' | '\\' | '\0') || (c as u32) < 32) {
        return raw;
    }
    let collapse = |t: &str| -> String {
        let mut t = t.to_string();
        while t.contains("--") {
            t = t.replace("--", "-");
        }
        t
    };
    let t = s.trim().to_lowercase().replace('_', "-");
    let t = t.split_whitespace().collect::<Vec<_>>().join("-");
    let t = collapse(&t);
    let t: String = t.trim_matches('-').chars().filter(|c| c.is_alphanumeric() || *c == '-').collect();
    let t = collapse(&t);
    let t = t.trim_matches('-').to_string();
    if t.is_empty() {
        raw
    } else {
        t
    }
}

/// Every fold a K2 sender may have applied to `s` (non-empty, deduped).
pub fn name_folds(s: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(4);
    for f in [
        s.trim().to_lowercase(),
        normalize_address_token(s),
        dash_fold_name(s),
        cli_fold_name(s),
    ] {
        if !f.is_empty() && !out.contains(&f) {
            out.push(f);
        }
    }
    out
}

/// True when `a` and `b` name the same thing under any sender's fold.
/// Different tokens (`sales` vs `sales-team`) still do not match (D20).
pub fn names_loosely_match(a: &str, b: &str) -> bool {
    let fb = name_folds(b);
    name_folds(a).iter().any(|x| fb.contains(x))
}

/// Every name the workspace `project_id` answers to from its own DB rows:
/// `projects.handle`, its derived address name, every alias, and
/// `projects.name`. The caller adds `resolve_agent_name` (which takes the
/// DB lock itself, so it can't run under `conn`).
pub fn workspace_names(conn: &Connection, project_id: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |s: String| {
        let s = s.trim().to_string();
        if !s.is_empty() && !out.contains(&s) {
            out.push(s);
        }
    };
    if let Ok((handle, name)) = conn.query_row(
        "SELECT handle, name FROM projects WHERE id = ?1",
        params![project_id],
        |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, String>(1)?)),
    ) {
        if let Some(h) = handle {
            push(h);
        }
        push(name);
    } else {
        return Vec::new();
    }
    if let Ok(addr) = project_handle(conn, project_id) {
        push(addr);
    }
    for a in aliases_for(conn, project_id) {
        push(a);
    }
    out
}

/// Which stored remote rows one peer's roster may touch (CA6).
///
/// A roster is signed by ONE peer, so it may only heal rows that point at
/// that peer: rows whose host routes to it, and whose `peer_fingerprint`
/// is empty or that peer's. Without this, peer A's roster could rename
/// (or delete, on a UNIQUE clash) our rows that point at host B.
pub struct RosterPeerScope<'a> {
    /// The roster's peer fingerprint (verified pin).
    pub fingerprint: &'a str,
    /// True when a stored row's `host` routes to this peer.
    pub host_matches: &'a dyn Fn(&str) -> bool,
}

impl RosterPeerScope<'_> {
    /// Whether a stored `(host, peer_fingerprint)` belongs to this peer.
    pub fn owns_row(&self, host: &str, peer_fingerprint: Option<&str>) -> bool {
        let fp_ok = match peer_fingerprint.map(str::trim).filter(|s| !s.is_empty()) {
            None => true,
            Some(fp) => !self.fingerprint.trim().is_empty() && fp == self.fingerprint.trim(),
        };
        fp_ok && (self.host_matches)(host)
    }
}

/// One entry of a peer's signed roster, as the heal needs it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RosterEntry {
    /// The peer's `projects.id` for this workspace.
    pub workspace_id: String,
    /// Roster `agent` — the workspace handle (D8).
    pub handle: String,
    /// Previous handles / pre-slug names (D8).
    pub aliases: Vec<String>,
    /// Roster `workspace_name` — the display name (not unique, D15).
    pub workspace_name: String,
}

/// What one roster heal changed (logs and tests).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HealReport {
    /// Rows newly bound to a workspace id.
    pub bound: usize,
    /// Rows whose bound id left the roster and were cleared (CA9).
    pub cleared: usize,
    /// Rows whose `agent` was rewritten from an alias to the handle (D10).
    pub renamed: usize,
}

/// How a stored row's name matched a roster entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RosterMatch {
    Handle,
    Alias,
    Display,
}

/// The roster entry a stored connection name refers to, if exactly one.
/// Order: handle, then alias, then display name; a display name binds only
/// when one workspace on that peer has it (D15). Different tokens never
/// match (D20).
fn match_roster_entry<'a>(
    agent: &str,
    roster: &'a [RosterEntry],
) -> Option<(&'a RosterEntry, RosterMatch)> {
    fn unique<'a>(hits: Vec<&'a RosterEntry>) -> Option<&'a RosterEntry> {
        let first = *hits.first()?;
        hits.iter()
            .all(|e| e.workspace_id == first.workspace_id)
            .then_some(first)
    }
    let by_handle: Vec<&RosterEntry> = roster
        .iter()
        .filter(|e| !e.handle.trim().is_empty() && names_loosely_match(agent, &e.handle))
        .collect();
    if let Some(e) = unique(by_handle) {
        return Some((e, RosterMatch::Handle));
    }
    let by_alias: Vec<&RosterEntry> = roster
        .iter()
        .filter(|e| e.aliases.iter().any(|a| names_loosely_match(agent, a)))
        .collect();
    if let Some(e) = unique(by_alias) {
        return Some((e, RosterMatch::Alias));
    }
    let by_display: Vec<&RosterEntry> = roster
        .iter()
        .filter(|e| {
            !e.workspace_name.trim().is_empty() && names_loosely_match(agent, &e.workspace_name)
        })
        .collect();
    unique(by_display).map(|e| (e, RosterMatch::Display))
}

/// Roster entries whose display name a stored connection name matches.
/// More than one workspace → the row can't bind (D15); the send hint
/// lists them.
pub fn roster_display_matches<'a>(agent: &str, roster: &'a [RosterEntry]) -> Vec<&'a RosterEntry> {
    let mut out: Vec<&RosterEntry> = Vec::new();
    for e in roster {
        if !e.workspace_name.trim().is_empty()
            && names_loosely_match(agent, &e.workspace_name)
            && !out.iter().any(|o| o.workspace_id == e.workspace_id)
        {
            out.push(e);
        }
    }
    out
}

/// Heal and bind this peer's stored remote rows from its roster (A7 S2).
///
/// Only rows this peer owns are touched ([`RosterPeerScope`], CA6). For
/// each one:
///
/// - **Bound** and the id is still in the roster → refresh the cached
///   handle / display name; keep the binding.
/// - **Bound** but the id left the roster (the peer re-registered the
///   workspace, e.g. after `k2 migrate`) → clear it and re-match (CA9).
///   The caller only passes a roster from a SUCCESSFUL fetch, so a failed
///   fetch never clears anything.
/// - **Unbound** → match the stored name against handle, alias, then
///   display name ([`match_roster_entry`]) and bind to that workspace id.
///
/// On a handle or alias match whose spelling differs from the handle, the
/// row's `agent` is rewritten to the handle (D10). A display-name match
/// binds without rewriting, so `k2 connections remove <name>::host` keeps
/// working. A rename that clashes with another row of the same workspace
/// keeps both rows and logs; nothing is ever deleted. An empty
/// `peer_fingerprint` is filled with the peer's.
pub fn heal_remote_connections_from_roster(
    conn: &Connection,
    scope: &RosterPeerScope<'_>,
    roster: &[RosterEntry],
) -> HealReport {
    use crate::db::schema::WorkspaceRemoteConnection;
    let mut report = HealReport::default();
    let rows = match WorkspaceRemoteConnection::list_all(conn) {
        Ok(r) => r,
        Err(e) => {
            crate::log_debug!("[handle] roster heal: list rows failed: {e}");
            return report;
        }
    };
    let fp = scope.fingerprint.trim();
    for row in rows {
        if !scope.owns_row(&row.host, row.peer_fingerprint.as_deref()) {
            continue;
        }
        if !fp.is_empty()
            && row
                .peer_fingerprint
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .is_none()
        {
            let _ = WorkspaceRemoteConnection::set_peer_fingerprint(conn, &row.id, Some(fp));
        }

        let bound_id = row
            .remote_workspace_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        let (entry, how) = match bound_id {
            Some(ref id) => match roster.iter().find(|e| &e.workspace_id == id) {
                Some(e) => {
                    // Still listed: the bound id wins over any name match.
                    let how = if names_loosely_match(&row.agent, &e.handle) {
                        RosterMatch::Handle
                    } else if e.aliases.iter().any(|a| names_loosely_match(&row.agent, a)) {
                        RosterMatch::Alias
                    } else {
                        RosterMatch::Display
                    };
                    (e, how)
                }
                None => {
                    // CA9: the peer no longer lists the bound workspace.
                    let _ = WorkspaceRemoteConnection::set_binding(conn, &row.id, None, None, None);
                    report.cleared += 1;
                    crate::log_debug!(
                        "[handle] roster heal {}: bound workspace {id} left {}'s roster; re-matching {}",
                        row.id,
                        row.host,
                        row.agent
                    );
                    match match_roster_entry(&row.agent, roster) {
                        Some(m) => m,
                        None => continue,
                    }
                }
            },
            None => match match_roster_entry(&row.agent, roster) {
                Some(m) => m,
                None => continue,
            },
        };

        let newly_bound = row.remote_workspace_id.as_deref() != Some(entry.workspace_id.as_str());
        let display = entry.workspace_name.trim();
        if let Err(e) = WorkspaceRemoteConnection::set_binding(
            conn,
            &row.id,
            Some(&entry.workspace_id),
            Some(entry.handle.trim()),
            (!display.is_empty()).then_some(display),
        ) {
            crate::log_debug!("[handle] roster heal {}: bind failed: {e}", row.id);
            continue;
        }
        if newly_bound {
            report.bound += 1;
        }

        // D10: an alias (or a differently spelled handle) heals to the
        // handle. A display-name match keeps the name the owner typed.
        let canonical = entry.handle.trim().to_ascii_lowercase();
        if how == RosterMatch::Display || canonical.is_empty() || row.agent == canonical {
            continue;
        }
        let new_addr = if row.remote_addr.contains("::") || !row.remote_addr.contains('@') {
            format!("{canonical}::{}", row.host)
        } else {
            format!("{canonical}@{}", row.host)
        };
        match conn.execute(
            "UPDATE workspace_remote_connections SET agent = ?1, remote_addr = ?2 WHERE id = ?3",
            params![canonical, new_addr, row.id],
        ) {
            Ok(_) => report.renamed += 1,
            Err(e) if e.to_string().contains("UNIQUE") => {
                // The workspace already has a row under the handle. Keep
                // both (both are bound to the same id); never delete a
                // connection because a peer's roster said so.
                crate::log_debug!(
                    "[handle] roster heal {}: {}::{} → {canonical} clashes with an existing row; kept both",
                    row.id,
                    row.agent,
                    row.host
                );
            }
            Err(e) => crate::log_debug!("[handle] roster heal {} rename failed: {e}", row.id),
        }
    }
    report
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

    /// CA2: each sender fold, pinned to what the CLI / tray resolver / address
    /// token produce today.
    #[test]
    fn sender_folds_match_live_normalizers() {
        assert_eq!(dash_fold_name(" Seoca.Old_x "), "seoca-old-x");
        assert_eq!(cli_fold_name(" Seoca.Old_x "), "seocaold-x");
        assert_eq!(cli_fold_name("Press  Agent"), "press-agent");
        assert_eq!(cli_fold_name("a/b"), "a/b", "path chars keep the raw lowercase form");
        assert_eq!(cli_fold_name("..."), "...", "empty slug falls back to raw");
        assert_eq!(normalize_address_token("Seoca.Old_x"), "seocaold-x");
        assert_eq!(
            name_folds("Seoca.Old"),
            vec!["seoca.old".to_string(), "seocaold".to_string(), "seoca-old".to_string()]
        );
    }

    #[test]
    fn names_loosely_match_crosses_folds_but_not_d20() {
        assert!(names_loosely_match("seoca.old", "seoca-old"), "tray fold");
        assert!(names_loosely_match("seoca.old", "seocaold"), "cli fold");
        assert!(names_loosely_match("Press Agent", "press-agent"));
        assert!(names_loosely_match("QUILLIFY-WEBSITE", "quillify-website"));
        assert!(!names_loosely_match("sales", "sales-team"), "D20");
        assert!(!names_loosely_match("", ""), "empty never matches");
        assert!(!names_loosely_match("seoca", "quillify-website"));
    }

    #[test]
    fn workspace_names_lists_handle_name_and_aliases() {
        db::init_for_tests();
        let path = format!("/tmp/k2-wsnames-{}", uuid::Uuid::new_v4());
        let id = uuid::Uuid::new_v4().to_string();
        let u = &id[..8];
        let handle = format!("wsn-{u}");
        let alias = format!("wsn-old-{u}");
        {
            let dbh = db::shared();
            let conn = dbh.lock();
            conn.execute(
                "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?4)",
                params![id, "WSN Display", path, handle],
            )
            .unwrap();
            insert_alias_or_ignore(&conn, &id, &alias);
            let names = workspace_names(&conn, &id);
            assert_eq!(names, vec![handle.clone(), "WSN Display".to_string(), alias.clone()]);
            assert!(workspace_names(&conn, "no-such-id").is_empty());
        }
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
            let host_ok = |h: &str| h == "peer.k2.dev";
            heal_remote_connections_from_roster(
                &conn,
                &RosterPeerScope { fingerprint: "fp-peer", host_matches: &host_ok },
                &[entry("ws-sales-team", "sales-team", &["sales team"], "")],
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

    /// CA6 / F2: peer A's roster never rewrites a row that points at host
    /// B, nor a row bound to another peer's fingerprint on the same host.
    #[test]
    fn lazy_heal_is_scoped_to_the_rosters_peer() {
        crate::db::init_for_tests();
        let (id, path) = unique_dir("heal-scope");
        insert_project(&id, "Heal Scope", &path);
        // A second source workspace: one workspace can't hold two rows with
        // the same address.
        let (id2, path2) = unique_dir("heal-scope-2");
        insert_project(&id2, "Heal Scope Two", &path2);
        {
            let dbh = db::shared();
            let conn = dbh.lock();
            let mk = |src: &str, row_id: &str, agent: &str, host: &str, fp: Option<&str>| {
                crate::db::schema::WorkspaceRemoteConnection::create(
                    &conn,
                    row_id,
                    src,
                    &format!("{agent}::{host}"),
                    host,
                    agent,
                    fp,
                )
                .unwrap();
            };
            mk(&id, "scope-other-host", "sales-old", "scope-b.k2.dev", None);
            mk(&id2, "scope-other-fp", "sales-old", "scope-a.k2.dev", Some("fp-someone-else"));
            mk(&id, "scope-own", "sales-old", "scope-a.k2.dev", Some("fp-a"));
            let host_ok = |h: &str| h == "scope-a.k2.dev";
            heal_remote_connections_from_roster(
                &conn,
                &RosterPeerScope { fingerprint: "fp-a", host_matches: &host_ok },
                &[entry("ws-ceo", "ceo", &["sales-old"], "")],
            );
            let mut rows = crate::db::schema::WorkspaceRemoteConnection::list_for_source(&conn, &id)
                .unwrap();
            rows.extend(
                crate::db::schema::WorkspaceRemoteConnection::list_for_source(&conn, &id2).unwrap(),
            );
            let get = |row_id: &str| rows.iter().find(|r| r.id == row_id).expect(row_id).clone();
            assert_eq!(get("scope-other-host").agent, "sales-old", "host B row untouched");
            assert_eq!(get("scope-other-host").remote_addr, "sales-old::scope-b.k2.dev");
            assert_eq!(get("scope-other-fp").agent, "sales-old", "other peer's row untouched");
            assert_eq!(get("scope-own").agent, "ceo", "own peer's alias row heals");
        }
        std::fs::remove_dir_all(&path).ok();
        std::fs::remove_dir_all(&path2).ok();
    }

    /// CA6: a heal rename that clashes with an existing row keeps both.
    #[test]
    fn lazy_heal_clash_keeps_both_rows() {
        crate::db::init_for_tests();
        let (id, path) = unique_dir("heal-clash");
        insert_project(&id, "Heal Clash", &path);
        {
            let dbh = db::shared();
            let conn = dbh.lock();
            for (row_id, agent) in [("clash-canonical", "sales"), ("clash-alias", "sales-old")] {
                crate::db::schema::WorkspaceRemoteConnection::create(
                    &conn,
                    row_id,
                    &id,
                    &format!("{agent}::clash-a.k2.dev"),
                    "clash-a.k2.dev",
                    agent,
                    None,
                )
                .unwrap();
            }
            let host_ok = |h: &str| h == "clash-a.k2.dev";
            heal_remote_connections_from_roster(
                &conn,
                &RosterPeerScope { fingerprint: "fp-a", host_matches: &host_ok },
                &[entry("ws-sales", "sales", &["sales-old"], "")],
            );
            let rows = crate::db::schema::WorkspaceRemoteConnection::list_for_source(&conn, &id)
                .unwrap();
            assert_eq!(rows.len(), 2, "clash must never delete a row: {rows:?}");
            let alias = rows.iter().find(|r| r.id == "clash-alias").expect("alias row kept");
            assert_eq!(alias.agent, "sales-old", "clashing row is left as it was");
            assert_eq!(
                alias.remote_workspace_id.as_deref(),
                Some("ws-sales"),
                "a clashing row still binds to the workspace"
            );
        }
        std::fs::remove_dir_all(&path).ok();
    }

    fn entry(ws: &str, handle: &str, aliases: &[&str], display: &str) -> RosterEntry {
        RosterEntry {
            workspace_id: ws.to_string(),
            handle: handle.to_string(),
            aliases: aliases.iter().map(|s| s.to_string()).collect(),
            workspace_name: display.to_string(),
        }
    }

    /// One remote row on `<label>.k2.dev` for a fresh source workspace
    /// (a host per test: the heal touches every row of the peer's host,
    /// and the test DB is shared by parallel tests).
    fn one_row(label: &str, agent: &str) -> (String, String, String) {
        let host = format!("{label}.k2.dev");
        let (id, path) = unique_dir(label);
        insert_project(&id, &format!("A7 {label}"), &path);
        let row_id = format!("row-{}", uuid::Uuid::new_v4());
        let dbh = db::shared();
        let conn = dbh.lock();
        crate::db::schema::WorkspaceRemoteConnection::create(
            &conn,
            &row_id,
            &id,
            &format!("{agent}::{host}"),
            &host,
            agent,
            None,
        )
        .unwrap();
        (id, path, row_id)
    }

    fn heal_a(label: &str, roster: &[RosterEntry]) -> HealReport {
        let host = format!("{label}.k2.dev");
        let host_ok = |h: &str| h == host;
        let dbh = db::shared();
        let conn = dbh.lock();
        heal_remote_connections_from_roster(
            &conn,
            &RosterPeerScope { fingerprint: "fp-a", host_matches: &host_ok },
            roster,
        )
    }

    fn row(source: &str, row_id: &str) -> crate::db::schema::WorkspaceRemoteConnection {
        let dbh = db::shared();
        let conn = dbh.lock();
        crate::db::schema::WorkspaceRemoteConnection::list_for_source(&conn, source)
            .unwrap()
            .into_iter()
            .find(|r| r.id == row_id)
            .expect("row")
    }

    /// A7 S2 (PRD test 1, core half): a row saved under the display name
    /// binds to that workspace without rewriting the saved name, and the
    /// peer's fingerprint is filled in.
    #[test]
    fn roster_heal_binds_display_name_row_keeping_agent() {
        crate::db::init_for_tests();
        let (src, path, rid) = one_row("bind-display", "seoca");
        let report = heal_a("bind-display", &[
            entry("ws-w", "quillify-website", &[], "Seoca"),
            entry("ws-x", "other-site", &[], "Other"),
        ]);
        assert!(report.bound >= 1, "{report:?}");
        let r = row(&src, &rid);
        assert_eq!(r.remote_workspace_id.as_deref(), Some("ws-w"));
        assert_eq!(r.remote_handle.as_deref(), Some("quillify-website"));
        assert_eq!(r.remote_display_name.as_deref(), Some("Seoca"));
        assert_eq!(r.agent, "seoca", "a display-name bind never rewrites the saved name");
        assert_eq!(r.peer_fingerprint.as_deref(), Some("fp-a"));
        std::fs::remove_dir_all(&path).ok();
    }

    /// D15: a display name two workspaces share never binds.
    #[test]
    fn roster_heal_leaves_ambiguous_display_name_unbound() {
        crate::db::init_for_tests();
        let (src, path, rid) = one_row("ambig-display", "seoca");
        heal_a("ambig-display", &[
            entry("ws-1", "seoca-one", &[], "Seoca"),
            entry("ws-2", "seoca-two", &[], "Seoca"),
        ]);
        assert!(row(&src, &rid).remote_workspace_id.is_none());
        std::fs::remove_dir_all(&path).ok();
    }

    /// CA7: a row already spelled as the handle still binds; a handle
    /// match outranks another workspace's display name.
    #[test]
    fn roster_heal_binds_canonical_row_and_prefers_handle() {
        crate::db::init_for_tests();
        let (src, path, rid) = one_row("canonical", "seoca");
        heal_a("canonical", &[
            entry("ws-display", "quillify-website", &[], "Seoca"),
            entry("ws-handle", "seoca", &[], "Somebody"),
        ]);
        let r = row(&src, &rid);
        assert_eq!(r.remote_workspace_id.as_deref(), Some("ws-handle"));
        assert_eq!(r.agent, "seoca");
        std::fs::remove_dir_all(&path).ok();
    }

    /// CA9: the bound id left the roster (the peer re-registered the
    /// workspace) → cleared and re-matched by the saved name.
    #[test]
    fn roster_heal_rebinds_when_bound_id_leaves_the_roster() {
        crate::db::init_for_tests();
        let (src, path, rid) = one_row("rebind", "seoca");
        heal_a("rebind", &[entry("ws-old", "quillify-website", &[], "Seoca")]);
        assert_eq!(row(&src, &rid).remote_workspace_id.as_deref(), Some("ws-old"));
        let report = heal_a("rebind", &[entry("ws-new", "quillify-website", &[], "Seoca")]);
        assert!(report.cleared >= 1, "{report:?}");
        assert_eq!(row(&src, &rid).remote_workspace_id.as_deref(), Some("ws-new"));
        // Listed again under a new display name: the binding (by id) holds
        // and the cached names follow.
        heal_a("rebind", &[entry("ws-new", "quillify-website", &[], "Seoca Two")]);
        let r = row(&src, &rid);
        assert_eq!(r.remote_workspace_id.as_deref(), Some("ws-new"));
        assert_eq!(r.remote_display_name.as_deref(), Some("Seoca Two"));
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
