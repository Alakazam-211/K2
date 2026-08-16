//! Durable sidecar handles (`sales/1`, `sales/reviewer`).
//!
//! Extra harness sessions in a workspace are sidecars of the workspace
//! agent. Canonical (pinned) chat is **not** stored here — its address
//! is just the workspace name.
//!
//! Handle SSOT:
//! - If Chats `custom_name` is set and slugs to a valid address token →
//!   that slug (`Reviewer` → `reviewer`).
//! - Else a durable decimal ordinal starting at 1, never compacted
//!   when tabs close (v1 never recycles).
//!
//! `conversation_key` prefers the provider conversation id (claude
//! `--resume` uuid / `workspace_tab_sessions.session_id`). Until that
//! id is known we key on `pane_group_id` and rekey on first stamp.

use rusqlite::{params, Connection};

use crate::workspace::provider_resume::provider_resume_for_command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionHandleRow {
    pub project_id: String,
    pub conversation_key: String,
    pub ordinal: u32,
}

/// Slug a Chats `custom_name` for use as a `workspace/handle` token.
///
/// Rules: trim, lowercase, whitespace → `-`. Reject empty, `/`, `:`,
/// and other pathy junk (`\`, NUL, C0 controls). Fail-loud.
pub fn slugify_custom_name(name: &str) -> Result<String, String> {
    if name.chars().any(|c| {
        c == '/' || c == ':' || c == '\\' || c == '\0' || c.is_control()
    }) {
        return Err(
            "chat name cannot contain '/', ':', or other path characters when used as an address"
                .to_string(),
        );
    }
    let slug = name
        .trim()
        .to_lowercase()
        .split_whitespace()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if slug.is_empty() {
        return Err("chat name is empty after slugify".to_string());
    }
    if slug.contains('/') || slug.contains(':') {
        return Err("chat name slug cannot contain '/' or ':'".to_string());
    }
    Ok(slug)
}

/// True when `agent_name` is the workspace's canonical (pinned) slot.
pub fn is_canonical_agent_name(agent_name: &str, project_id: &str) -> bool {
    !project_id.is_empty() && agent_name == project_id
}

/// Extra-tab map key (`tab-<pane_group_id>`).
pub fn is_tab_agent_name(agent_name: &str) -> bool {
    agent_name.starts_with("tab-")
}

/// API host-session map key (`api-<principal>-<uuid>`).
pub fn is_api_agent_name(agent_name: &str) -> bool {
    agent_name.starts_with("api-")
}

/// Command is a known harness (claude/grok/pi/codex/gemini/cursor/hermes).
pub fn is_harness_command(command: Option<&str>) -> bool {
    command
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .and_then(provider_resume_for_command)
        .is_some()
}

/// Sidecar = extra GUI harness tab (`tab-*` + known CLI). Not a blank
/// shell, file tab, canonical pinned chat, heartbeat name, or `/v1`
/// host-session (`api-*` — those are relations, D16; they keep their
/// own `K2_SESSION_ID`).
pub fn is_sidecar_harness(agent_name: &str, project_id: &str, command: Option<&str>) -> bool {
    if is_canonical_agent_name(agent_name, project_id) {
        return false;
    }
    if is_api_agent_name(agent_name) {
        return false;
    }
    if !is_tab_agent_name(agent_name) {
        return false;
    }
    is_harness_command(command)
}

/// Strip the `tab-` map-key prefix so spawn and wake share one pane key.
pub fn normalize_pane_key(pane_or_tab_key: &str) -> &str {
    let trimmed = pane_or_tab_key.trim();
    trimmed.strip_prefix("tab-").unwrap_or(trimmed)
}

/// Durable conversation key: provider session id when known, else the
/// tab/pane key (`tab-xyz` and `xyz` are the same key).
pub fn conversation_key_for(provider_session_id: Option<&str>, pane_or_tab_key: &str) -> String {
    match provider_session_id.map(str::trim).filter(|s| !s.is_empty()) {
        Some(sid) => sid.to_string(),
        None => normalize_pane_key(pane_or_tab_key).to_string(),
    }
}

/// Return the existing ordinal or issue the next unused integer (MAX+1).
/// Does not fill holes. Resume / re-wake of the same key does not increment.
pub fn allocate_ordinal(
    conn: &Connection,
    project_id: &str,
    conversation_key: &str,
) -> Result<u32, String> {
    let project_id = project_id.trim();
    let conversation_key = conversation_key.trim();
    if project_id.is_empty() {
        return Err("allocate_ordinal: project_id required".to_string());
    }
    if conversation_key.is_empty() {
        return Err("allocate_ordinal: conversation_key required".to_string());
    }
    if let Some(row) = get(conn, project_id, conversation_key)? {
        return Ok(row.ordinal);
    }
    let max: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(ordinal), 0) FROM workspace_session_handles WHERE project_id = ?1",
            params![project_id],
            |r| r.get(0),
        )
        .map_err(|e| format!("allocate_ordinal max: {e}"))?;
    let next = u32::try_from(max + 1).map_err(|_| "allocate_ordinal: ordinal overflow".to_string())?;
    conn.execute(
        "INSERT INTO workspace_session_handles (project_id, conversation_key, ordinal) \
         VALUES (?1, ?2, ?3)",
        params![project_id, conversation_key, next as i64],
    )
    .map_err(|e| format!("allocate_ordinal insert: {e}"))?;
    Ok(next)
}

/// When a provider session id arrives after we keyed on pane_group_id,
/// move the row so resume uses the same ordinal. No-op if already keyed
/// on `new_key` or `old_key` has no row.
pub fn rekey_conversation(
    conn: &Connection,
    project_id: &str,
    old_key: &str,
    new_key: &str,
) -> Result<(), String> {
    let project_id = project_id.trim();
    let old_key = old_key.trim();
    let new_key = new_key.trim();
    if project_id.is_empty() || old_key.is_empty() || new_key.is_empty() {
        return Err("rekey_conversation: project_id and keys required".to_string());
    }
    if old_key == new_key {
        return Ok(());
    }
    if get(conn, project_id, new_key)?.is_some() {
        return Ok(());
    }
    let n = conn
        .execute(
            "UPDATE workspace_session_handles SET conversation_key = ?3 \
             WHERE project_id = ?1 AND conversation_key = ?2",
            params![project_id, old_key, new_key],
        )
        .map_err(|e| format!("rekey_conversation: {e}"))?;
    let _ = n;
    Ok(())
}

pub fn get(
    conn: &Connection,
    project_id: &str,
    conversation_key: &str,
) -> Result<Option<SessionHandleRow>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT project_id, conversation_key, ordinal \
             FROM workspace_session_handles \
             WHERE project_id = ?1 AND conversation_key = ?2",
        )
        .map_err(|e| e.to_string())?;
    let mut rows = stmt
        .query_map(params![project_id, conversation_key], |r| {
            Ok(SessionHandleRow {
                project_id: r.get(0)?,
                conversation_key: r.get(1)?,
                ordinal: r.get::<_, i64>(2)? as u32,
            })
        })
        .map_err(|e| e.to_string())?;
    match rows.next() {
        Some(row) => Ok(Some(row.map_err(|e| e.to_string())?)),
        None => Ok(None),
    }
}

pub fn get_by_ordinal(
    conn: &Connection,
    project_id: &str,
    ordinal: u32,
) -> Result<Option<SessionHandleRow>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT project_id, conversation_key, ordinal \
             FROM workspace_session_handles \
             WHERE project_id = ?1 AND ordinal = ?2",
        )
        .map_err(|e| e.to_string())?;
    let mut rows = stmt
        .query_map(params![project_id, ordinal as i64], |r| {
            Ok(SessionHandleRow {
                project_id: r.get(0)?,
                conversation_key: r.get(1)?,
                ordinal: r.get::<_, i64>(2)? as u32,
            })
        })
        .map_err(|e| e.to_string())?;
    match rows.next() {
        Some(row) => Ok(Some(row.map_err(|e| e.to_string())?)),
        None => Ok(None),
    }
}

/// Custom name for a provider conversation id (any provider row).
pub fn custom_name_for_session_id(
    conn: &Connection,
    session_id: &str,
) -> Result<Option<String>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT custom_name FROM chat_session_names \
             WHERE session_id = ?1 AND TRIM(custom_name) != '' \
             ORDER BY updated_at DESC LIMIT 1",
        )
        .map_err(|e| e.to_string())?;
    let mut rows = stmt
        .query_map(params![session_id], |r| r.get::<_, String>(0))
        .map_err(|e| e.to_string())?;
    match rows.next() {
        Some(Ok(name)) => {
            let trimmed = name.trim();
            if trimmed.is_empty() {
                Ok(None)
            } else {
                Ok(Some(trimmed.to_string()))
            }
        }
        Some(Err(e)) => Err(e.to_string()),
        None => Ok(None),
    }
}

/// True when this conversation has a valid custom-name slug (ordinal
/// must then fail loud).
pub fn has_valid_custom_slug(conn: &Connection, conversation_key: &str) -> Result<bool, String> {
    match custom_name_for_session_id(conn, conversation_key)? {
        Some(name) => Ok(slugify_custom_name(&name).is_ok()),
        None => Ok(false),
    }
}

/// Address token for a sidecar session: slug if custom_name is valid,
/// else the durable ordinal (allocating if needed).
pub fn handle_for_session(
    conn: &Connection,
    project_id: &str,
    conversation_key: &str,
    provider_session_id: Option<&str>,
) -> Result<String, String> {
    let name_key = provider_session_id
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(conversation_key);
    if let Some(name) = custom_name_for_session_id(conn, name_key)? {
        if let Ok(slug) = slugify_custom_name(&name) {
            return Ok(slug);
        }
    }
    let ordinal = allocate_ordinal(conn, project_id, conversation_key)?;
    Ok(ordinal.to_string())
}

/// Resolve `handle` (`1` / `reviewer`) to a conversation_key.
/// Slug match on custom_name wins. After a rename the old ordinal
/// fails loud. Clearing the name lets the ordinal work again.
pub fn resolve_handle(
    conn: &Connection,
    project_id: &str,
    handle: &str,
) -> Result<String, String> {
    let handle = handle.trim();
    if handle.is_empty() {
        return Err("empty sidecar handle".to_string());
    }
    if handle.contains('/') || handle.contains(':') {
        return Err(format!(
            "invalid sidecar handle '{handle}' — use workspace/handle with a single slash"
        ));
    }

    if let Some(key) = find_conversation_by_slug(conn, project_id, handle)? {
        return Ok(key);
    }

    if let Ok(ordinal) = handle.parse::<u32>() {
        if ordinal == 0 {
            return Err("sidecar ordinals start at 1".to_string());
        }
        match get_by_ordinal(conn, project_id, ordinal)? {
            Some(row) => {
                if has_valid_custom_slug(conn, &row.conversation_key)? {
                    return Err(format!(
                        "handle '{ordinal}' was replaced by a Chats name — use the slug, not the old ordinal"
                    ));
                }
                Ok(row.conversation_key)
            }
            None => Err(format!("unknown sidecar handle '{handle}' in this workspace")),
        }
    } else {
        Err(format!("unknown sidecar handle '{handle}' in this workspace"))
    }
}

fn find_conversation_by_slug(
    conn: &Connection,
    project_id: &str,
    slug: &str,
) -> Result<Option<String>, String> {
    let mut matches: Vec<String> = Vec::new();

    // Tab / API extra sessions (provider session_id on the tab row).
    let mut stmt = conn
        .prepare(
            "SELECT session_id, pane_group_id FROM workspace_tab_sessions \
             WHERE project_id = ?1",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![project_id], |r| {
            Ok((r.get::<_, Option<String>>(0)?, r.get::<_, String>(1)?))
        })
        .map_err(|e| e.to_string())?;
    for row in rows {
        let (session_id, pane) = row.map_err(|e| e.to_string())?;
        if let Some(sid) = session_id.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            if let Some(name) = custom_name_for_session_id(conn, sid)? {
                if let Ok(got) = slugify_custom_name(&name) {
                    if got == slug {
                        matches.push(sid.to_string());
                    }
                }
            }
        } else if let Some(name) = custom_name_for_session_id(conn, &pane)? {
            if let Ok(got) = slugify_custom_name(&name) {
                if got == slug {
                    matches.push(pane);
                }
            }
        }
    }

    // Handle-table keys that may already be provider ids.
    let mut stmt = conn
        .prepare(
            "SELECT conversation_key FROM workspace_session_handles WHERE project_id = ?1",
        )
        .map_err(|e| e.to_string())?;
    let keys = stmt
        .query_map(params![project_id], |r| r.get::<_, String>(0))
        .map_err(|e| e.to_string())?;
    for key in keys {
        let key = key.map_err(|e| e.to_string())?;
        if matches.iter().any(|m| m == &key) {
            continue;
        }
        if let Some(name) = custom_name_for_session_id(conn, &key)? {
            if let Ok(got) = slugify_custom_name(&name) {
                if got == slug {
                    matches.push(key);
                }
            }
        }
    }

    matches.sort();
    matches.dedup();
    match matches.len() {
        0 => Ok(None),
        1 => Ok(Some(matches.remove(0))),
        _ => Err(format!(
            "sidecar slug '{slug}' matches more than one chat in this workspace"
        )),
    }
}

/// Fail the second rename in a workspace that would share a slug.
pub fn ensure_slug_unique_in_workspace(
    conn: &Connection,
    project_id: &str,
    session_id: &str,
    slug: &str,
) -> Result<(), String> {
    if let Some(existing) = find_conversation_by_slug(conn, project_id, slug)? {
        if existing != session_id {
            return Err(format!(
                "chat name slug '{slug}' is already used by another session in this workspace"
            ));
        }
    }
    Ok(())
}

/// Best-effort project_id for a provider/daemon session id.
pub fn project_id_for_session_id(
    conn: &Connection,
    session_id: &str,
) -> Result<Option<String>, String> {
    if let Ok(id) = conn.query_row(
        "SELECT project_id FROM workspace_tab_sessions WHERE session_id = ?1 LIMIT 1",
        params![session_id],
        |r| r.get::<_, String>(0),
    ) {
        return Ok(Some(id));
    }
    if let Ok(id) = conn.query_row(
        "SELECT project_id FROM workspace_sessions WHERE session_id = ?1 LIMIT 1",
        params![session_id],
        |r| r.get::<_, String>(0),
    ) {
        return Ok(Some(id));
    }
    if let Ok(id) = conn.query_row(
        "SELECT project_id FROM workspace_session_handles WHERE conversation_key = ?1 LIMIT 1",
        params![session_id],
        |r| r.get::<_, String>(0),
    ) {
        return Ok(Some(id));
    }
    Ok(None)
}

/// Workspace address token (`projects.name`, else folder basename).
pub fn workspace_address_name(conn: &Connection, project_id: &str) -> Result<String, String> {
    let (name, path): (String, String) = conn
        .query_row(
            "SELECT name, path FROM projects WHERE id = ?1",
            params![project_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|_| format!("workspace not found: {project_id}"))?;
    let name = name.trim();
    if !name.is_empty() && !name.contains('/') && !name.contains(':') {
        return Ok(name.to_string());
    }
    let base = std::path::Path::new(&path)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let base = base.trim();
    if base.is_empty() {
        Err("workspace has no addressable name".to_string())
    } else {
        Ok(base.to_string())
    }
}

/// Full address: `sales` (canonical) or `sales/reviewer` (sidecar).
pub fn format_address(workspace_name: &str, sidecar_handle: Option<&str>) -> String {
    match sidecar_handle.map(str::trim).filter(|s| !s.is_empty()) {
        Some(h) => format!("{workspace_name}/{h}"),
        None => workspace_name.to_string(),
    }
}

/// Shared-DB convenience wrappers (daemon + CLI-adjacent).
pub fn allocate_ordinal_shared(project_id: &str, conversation_key: &str) -> Result<u32, String> {
    let db = crate::db::shared();
    let conn = db.lock();
    allocate_ordinal(&conn, project_id, conversation_key)
}

pub fn handle_for_session_shared(
    project_id: &str,
    conversation_key: &str,
    provider_session_id: Option<&str>,
) -> Result<String, String> {
    let db = crate::db::shared();
    let conn = db.lock();
    handle_for_session(&conn, project_id, conversation_key, provider_session_id)
}

pub fn resolve_handle_shared(project_id: &str, handle: &str) -> Result<String, String> {
    let db = crate::db::shared();
    let conn = db.lock();
    resolve_handle(&conn, project_id, handle)
}

pub fn workspace_address_name_shared(project_id: &str) -> Result<String, String> {
    let db = crate::db::shared();
    let conn = db.lock();
    workspace_address_name(&conn, project_id)
}

/// Persist a sidecar handle when an extra harness session is first
/// registered. Resume of the same conversation_key does not increment.
pub fn ensure_sidecar_handle(
    conn: &Connection,
    project_id: &str,
    agent_name: &str,
    command: Option<&str>,
    provider_session_id: Option<&str>,
    pane_or_tab_key: &str,
) -> Result<Option<String>, String> {
    if !is_sidecar_harness(agent_name, project_id, command) {
        return Ok(None);
    }
    let key = conversation_key_for(provider_session_id, pane_or_tab_key);
    if key.is_empty() {
        return Err("ensure_sidecar_handle: empty conversation_key".to_string());
    }
    if let Some(sid) = provider_session_id.map(str::trim).filter(|s| !s.is_empty()) {
        let pane = normalize_pane_key(pane_or_tab_key);
        if pane != sid {
            rekey_conversation(conn, project_id, pane, sid)?;
        }
    }
    let handle = handle_for_session(conn, project_id, &key, provider_session_id)?;
    Ok(Some(handle))
}

pub fn ensure_sidecar_handle_shared(
    project_id: &str,
    agent_name: &str,
    command: Option<&str>,
    provider_session_id: Option<&str>,
    pane_or_tab_key: &str,
) -> Result<Option<String>, String> {
    let db = crate::db::shared();
    let conn = db.lock();
    ensure_sidecar_handle(
        &conn,
        project_id,
        agent_name,
        command,
        provider_session_id,
        pane_or_tab_key,
    )
}

/// Split a first-arg token into workspace + handle when it is a
/// `workspace/handle` form. Absolute paths (`/Users/...`) return None.
/// `sales-reviewer` (no slash) returns None. Federation `::` is not parsed.
pub fn split_workspace_handle(token: &str) -> Option<(&str, &str)> {
    let token = token.trim();
    if token.is_empty() || token.starts_with('/') {
        return None;
    }
    if token.contains("::") {
        return None;
    }
    let (ws, handle) = token.split_once('/')?;
    if ws.is_empty() || handle.is_empty() {
        return None;
    }
    Some((ws, handle))
}

/// UUID-shaped (36 chars, 4 dashes, hex). Same shape as projects.id
/// and daemon SessionId.
pub fn is_uuid_shape(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 36
        && b[8] == b'-'
        && b[13] == b'-'
        && b[18] == b'-'
        && b[23] == b'-'
        && s.bytes()
            .enumerate()
            .all(|(i, c)| matches!(i, 8 | 13 | 18 | 23) || c.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    fn conn() -> std::sync::Arc<parking_lot::ReentrantMutex<Connection>> {
        db::init_for_tests()
    }

    fn seed_project(name: &str) -> String {
        let dbh = conn();
        let c = dbh.lock();
        let id = uuid::Uuid::new_v4().to_string();
        let path = format!("/tmp/sidecar-handles-{name}-{id}");
        c.execute(
            "INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
            params![id, name, path],
        )
        .expect("seed project");
        id
    }

    fn insert_custom_name(session_id: &str, name: &str) {
        let dbh = conn();
        let c = dbh.lock();
        c.execute(
            "INSERT INTO chat_session_names (provider, session_id, custom_name, pinned, updated_at) \
             VALUES ('claude', ?1, ?2, 0, unixepoch()) \
             ON CONFLICT(provider, session_id) DO UPDATE SET custom_name = ?2, updated_at = unixepoch()",
            params![session_id, name],
        )
        .expect("insert custom_name");
    }

    fn insert_tab(project_id: &str, pane: &str, session_id: Option<&str>, command: &str) {
        let dbh = conn();
        let c = dbh.lock();
        c.execute(
            "INSERT INTO workspace_tab_sessions \
             (project_id, pane_group_id, agent_name, session_id, command, last_seen_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, unixepoch()) \
             ON CONFLICT(project_id, pane_group_id) DO UPDATE SET \
                session_id = excluded.session_id, command = excluded.command",
            params![
                project_id,
                pane,
                format!("tab-{pane}"),
                session_id,
                command,
            ],
        )
        .expect("insert tab");
    }

    #[test]
    fn slugify_reviewer_and_rejects_slash_colon() {
        assert_eq!(slugify_custom_name("Reviewer").expect("ok"), "reviewer");
        assert_eq!(
            slugify_custom_name("  Code Review  ").expect("ok"),
            "code-review"
        );
        slugify_custom_name("sales/reviewer").expect_err("slash");
        slugify_custom_name("sales:reviewer").expect_err("colon");
        slugify_custom_name("   ").expect_err("empty");
        slugify_custom_name("a\\b").expect_err("backslash");
    }

    #[test]
    fn sales_hyphen_reviewer_is_not_a_handle_parse() {
        assert!(split_workspace_handle("sales-reviewer").is_none());
        assert_eq!(
            split_workspace_handle("sales/reviewer"),
            Some(("sales", "reviewer"))
        );
        assert_eq!(split_workspace_handle("sales/1"), Some(("sales", "1")));
        assert!(split_workspace_handle("/Users/foo/sales").is_none());
        assert!(split_workspace_handle("agent::host").is_none());
        assert!(split_workspace_handle("sales/").is_none());
    }

    #[test]
    fn allocate_ordinal_increments_and_does_not_reuse() {
        let project_id = seed_project("sales");
        let dbh = conn();
        let c = dbh.lock();
        let a = allocate_ordinal(&c, &project_id, "pane-a").expect("first");
        let b = allocate_ordinal(&c, &project_id, "pane-b").expect("second");
        assert_eq!(a, 1, "first extra → 1");
        assert_eq!(b, 2, "second extra → 2");
        let a2 = allocate_ordinal(&c, &project_id, "pane-a").expect("resume");
        assert_eq!(a2, 1, "resume must not increment");
        // Closing a tab does not delete the row; next extra is 3, not 1.
        let d = allocate_ordinal(&c, &project_id, "pane-c").expect("third");
        assert_eq!(d, 3, "v1 never recycles / fills holes");
    }

    #[test]
    fn rename_to_reviewer_replaces_ordinal_and_old_one_fails() {
        let project_id = seed_project("sales-rn");
        let sid = format!("sid-reviewer-{}", uuid::Uuid::new_v4());
        insert_tab(&project_id, "pane-r", Some(&sid), "claude");
        let dbh = conn();
        let c = dbh.lock();
        let n = allocate_ordinal(&c, &project_id, &sid).expect("ord");
        assert_eq!(n, 1);
        assert_eq!(
            handle_for_session(&c, &project_id, &sid, Some(&sid)).expect("h"),
            "1"
        );
        drop(c);

        insert_custom_name(&sid, "Reviewer");

        let c = dbh.lock();
        assert_eq!(
            handle_for_session(&c, &project_id, &sid, Some(&sid)).expect("slug"),
            "reviewer"
        );
        let resolved = resolve_handle(&c, &project_id, "reviewer").expect("slug resolve");
        assert_eq!(resolved, sid);
        let old = resolve_handle(&c, &project_id, "1");
        assert!(
            old.is_err(),
            "old ordinal must fail loud after rename, got {old:?}"
        );
    }

    #[test]
    fn sidecar_classification() {
        let pid = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
        assert!(!is_sidecar_harness(pid, pid, Some("claude")));
        assert!(is_sidecar_harness("tab-xyz", pid, Some("claude")));
        assert!(is_sidecar_harness("tab-xyz", pid, Some("/usr/bin/grok")));
        assert!(!is_sidecar_harness("tab-xyz", pid, Some("zsh")));
        assert!(!is_sidecar_harness("tab-xyz", pid, None));
        assert!(
            !is_sidecar_harness("api-p-uuid", pid, Some("claude")),
            "API host-sessions are relations, not sidecars (D16)"
        );
        assert!(!is_sidecar_harness("daily-review", pid, Some("claude")));
    }

    #[test]
    fn tab_prefix_and_bare_pane_share_one_ordinal() {
        let project_id = seed_project("sales-pane");
        let dbh = conn();
        let c = dbh.lock();
        let a = allocate_ordinal(&c, &project_id, &conversation_key_for(None, "xyz")).expect("first");
        assert_eq!(a, 1);
        let b = allocate_ordinal(&c, &project_id, &conversation_key_for(None, "tab-xyz"))
            .expect("wake key");
        assert_eq!(b, 1, "wake with tab-xyz must reuse the pane-xyz ordinal");
    }

    #[test]
    fn second_rename_to_same_slug_fails_loud() {
        let project_id = seed_project("sales-uniq");
        let a = format!("sid-a-{}", uuid::Uuid::new_v4());
        let b = format!("sid-b-{}", uuid::Uuid::new_v4());
        insert_tab(&project_id, "pa", Some(&a), "claude");
        insert_tab(&project_id, "pb", Some(&b), "claude");
        insert_custom_name(&a, "Reviewer");
        let dbh = conn();
        let c = dbh.lock();
        crate::workspace_session_handles::ensure_slug_unique_in_workspace(
            &c, &project_id, &b, "reviewer",
        )
        .expect_err("second reviewer slug must fail");
    }

    #[test]
    fn is_uuid_shape_strict() {
        assert!(is_uuid_shape("a1b2c3d4-e5f6-7890-abcd-ef1234567890"));
        assert!(!is_uuid_shape("sales"));
        assert!(!is_uuid_shape("a1b2c3d4-e5f6-7890-abcd-ef1234567890/x"));
    }
}
