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

/// Slug a workspace display / handle candidate into an address token.
///
/// D4: **new helper** — do not change [`slugify_custom_name`] (live sidecar
/// addresses like `scout_v3` / `k2---marketing` must stay). Workspace
/// handles add `_` → `-` and hyphen-collapse so `K2 - Marketing` →
/// `k2-marketing`.
///
/// Rejects `/` `:` `\` NUL / C0 controls. Result must match
/// `^[a-z0-9]+(-[a-z0-9]+)*$`.
pub fn slugify_address_token(name: &str) -> Result<String, String> {
    if name.chars().any(|c| c == '/' || c == ':' || c == '\\' || c == '\0' || c.is_control()) {
        return Err(
            "workspace handle cannot contain '/', ':', or other path characters".to_string(),
        );
    }
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("workspace handle is empty".to_string());
    }
    let mut slug = trimmed.to_lowercase().replace('_', "-");
    slug = slug
        .split_whitespace()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    slug = collapse_hyphens(&slug);
    slug = slug
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();
    slug = collapse_hyphens(&slug);
    if slug.is_empty() {
        return Err("workspace handle is empty after slugify".to_string());
    }
    if !is_address_token(&slug) {
        return Err(format!(
            "workspace handle '{slug}' is not a valid address token (expected lowercase letters, digits, and single hyphens)"
        ));
    }
    Ok(slug)
}

fn collapse_hyphens(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_hyphen = false;
    for c in s.chars() {
        if c == '-' {
            if !prev_hyphen && !out.is_empty() {
                out.push('-');
            }
            prev_hyphen = true;
        } else {
            out.push(c);
            prev_hyphen = false;
        }
    }
    if out.ends_with('-') {
        out.pop();
    }
    out
}

/// `^[a-z0-9]+(-[a-z0-9]+)*$`
pub fn is_address_token(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    let mut chars = s.chars().peekable();
    let mut saw_alnum = false;
    while let Some(c) = chars.next() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            saw_alnum = true;
            continue;
        }
        if c == '-' {
            match chars.peek() {
                Some(n) if n.is_ascii_lowercase() || n.is_ascii_digit() => continue,
                _ => return false,
            }
        }
        return false;
    }
    saw_alnum && !s.starts_with('-') && !s.ends_with('-') && !s.contains("--")
}

/// D9/D10 matcher: slug if it succeeds, else trim + ASCII lowercase.
pub fn normalize_address_token(token: &str) -> String {
    match slugify_address_token(token) {
        Ok(s) => s,
        Err(_) => token.trim().to_ascii_lowercase(),
    }
}

/// True when two tokens slug-equate (`Sales Team` = `sales team` = `sales-team`).
/// Different tokens (`sales` vs `sales-team`) do **not** match (D20).
pub fn address_tokens_match(a: &str, b: &str) -> bool {
    normalize_address_token(a) == normalize_address_token(b)
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

/// True when `session_id` is this workspace's pinned conversation
/// (`workspace_sessions.session_id`). That row is the SSOT for
/// `k2 msg <workspace>` — it must not be addressed as `ws/N` even if
/// a `tab-*` harness also holds the same provider id (postal-bot
/// whoami vs K2_CELL, 2026-08-19).
pub fn conversation_is_canonical(
    conn: &Connection,
    project_id: &str,
    session_id: &str,
) -> bool {
    let sid = session_id.trim();
    if project_id.trim().is_empty() || sid.is_empty() {
        return false;
    }
    match crate::db::schema::WorkspaceSession::get(conn, project_id) {
        Ok(Some(row)) => {
            row.session_id
                .as_deref()
                .map(str::trim)
                .is_some_and(|saved| saved == sid)
        }
        _ => false,
    }
}

/// Shared-DB wrapper for [`conversation_is_canonical`].
pub fn conversation_is_canonical_shared(project_id: &str, session_id: &str) -> bool {
    let db = crate::db::shared();
    let conn = db.lock();
    conversation_is_canonical(&conn, project_id, session_id)
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
    // TR5: retired names follow the conversation onto its provider id,
    // in the same lock as the handle row.
    conn.execute(
        "UPDATE workspace_session_handle_aliases SET conversation_key = ?3 \
         WHERE project_id = ?1 AND conversation_key = ?2",
        params![project_id, old_key, new_key],
    )
    .map_err(|e| format!("rekey aliases: {e}"))?;
    conn.execute(
        "UPDATE OR IGNORE workspace_session_unclaimed_names SET conversation_key = ?3 \
         WHERE project_id = ?1 AND conversation_key = ?2",
        params![project_id, old_key, new_key],
    )
    .map_err(|e| format!("rekey unclaimed: {e}"))?;
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

/// True when this conversation has a Chats name that claims an address
/// (the ordinal then fails loud on the strict [`resolve_handle`]).
pub fn has_valid_custom_slug(conn: &Connection, conversation_key: &str) -> Result<bool, String> {
    Ok(address_slug_for(conn, None, conversation_key)?.is_some())
}

/// TR2: an all-digit token is only ever an ordinal, never a name.
pub fn is_ordinal_token(token: &str) -> bool {
    let t = token.trim();
    !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit())
}

/// True when `(project_id, conversation_key)` is a restored chat whose
/// name was taken while it was archived (TR6b): it keeps its Chats
/// label but answers only at its ordinal.
fn name_is_unclaimed(
    conn: &Connection,
    project_id: &str,
    conversation_key: &str,
) -> Result<bool, String> {
    conn.query_row(
        "SELECT COUNT(*) FROM workspace_session_unclaimed_names \
         WHERE project_id = ?1 AND conversation_key = ?2",
        params![project_id, conversation_key],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n > 0)
    .map_err(|e| format!("unclaimed name lookup: {e}"))
}

/// The address slug a conversation's Chats name claims, if any.
///
/// `None` when the chat has no name, the name does not slug, the name
/// is all digits (TR2: display only, the address stays the ordinal),
/// the chat is archived (TR6a: archive frees its name), or, with a
/// workspace, the name was left unclaimed on restore (TR6b).
pub fn address_slug_for(
    conn: &Connection,
    project_id: Option<&str>,
    conversation_key: &str,
) -> Result<Option<String>, String> {
    let key = conversation_key.trim();
    if key.is_empty() {
        return Ok(None);
    }
    let name: Option<String> = conn
        .query_row(
            "SELECT custom_name FROM chat_session_names \
             WHERE session_id = ?1 AND TRIM(custom_name) != '' AND archived = 0 \
             ORDER BY updated_at DESC LIMIT 1",
            params![key],
            |r| r.get::<_, String>(0),
        )
        .map(Some)
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            other => Err(other.to_string()),
        })?;
    let Some(name) = name else {
        return Ok(None);
    };
    let Ok(slug) = slugify_custom_name(&name) else {
        return Ok(None);
    };
    if is_ordinal_token(&slug) {
        return Ok(None);
    }
    if let Some(pid) = project_id.map(str::trim).filter(|p| !p.is_empty()) {
        if name_is_unclaimed(conn, pid, key)? {
            return Ok(None);
        }
    }
    Ok(Some(slug))
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
    if let Some(slug) = address_slug_for(conn, Some(project_id), name_key)? {
        return Ok(slug);
    }
    let ordinal = allocate_ordinal(conn, project_id, conversation_key)?;
    Ok(ordinal.to_string())
}

/// The handle a conversation answers at right now: its name slug, else
/// its ordinal. `None` when it has neither. Never allocates.
pub fn current_handle_for(
    conn: &Connection,
    project_id: &str,
    conversation_key: &str,
) -> Result<Option<String>, String> {
    if let Some(slug) = address_slug_for(conn, Some(project_id), conversation_key)? {
        return Ok(Some(slug));
    }
    Ok(get(conn, project_id, conversation_key)?.map(|row| row.ordinal.to_string()))
}

/// Full current address of a conversation in a workspace: the workspace
/// handle for the pinned Chat, `ws/<handle>` for a sidecar, else `None`.
pub fn current_address_for(
    conn: &Connection,
    project_id: &str,
    conversation_id: &str,
) -> Result<Option<String>, String> {
    let ws = workspace_address_name(conn, project_id)?;
    if conversation_is_canonical(conn, project_id, conversation_id) {
        return Ok(Some(ws));
    }
    Ok(current_handle_for(conn, project_id, conversation_id)?
        .map(|h| format_address(&ws, Some(&h))))
}

/// Every address token a conversation answered at before: its ordinal
/// (when the name now wins) plus each retired name. Current handle
/// excluded. Used to render old Thread rows under the current name (Q5).
pub fn past_handles_for(
    conn: &Connection,
    project_id: &str,
    conversation_key: &str,
) -> Result<Vec<String>, String> {
    let current = current_handle_for(conn, project_id, conversation_key)?;
    let mut out: Vec<String> = Vec::new();
    if let Some(row) = get(conn, project_id, conversation_key)? {
        out.push(row.ordinal.to_string());
    }
    let mut stmt = conn
        .prepare(
            "SELECT slug FROM workspace_session_handle_aliases \
             WHERE project_id = ?1 AND conversation_key = ?2 ORDER BY retired_at, slug",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![project_id, conversation_key], |r| r.get::<_, String>(0))
        .map_err(|e| e.to_string())?;
    for row in rows {
        out.push(row.map_err(|e| e.to_string())?);
    }
    out.retain(|h| Some(h) != current.as_ref());
    out.dedup();
    Ok(out)
}

/// The chat a retired name is reserved to, if any.
pub fn alias_owner(
    conn: &Connection,
    project_id: &str,
    slug: &str,
) -> Result<Option<String>, String> {
    conn.query_row(
        "SELECT conversation_key FROM workspace_session_handle_aliases \
         WHERE project_id = ?1 AND slug = ?2",
        params![project_id, slug.trim()],
        |r| r.get::<_, String>(0),
    )
    .map(Some)
    .or_else(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => Ok(None),
        other => Err(format!("alias lookup: {other}")),
    })
}

/// Record that `conversation_key` used to answer at `slug` in this
/// workspace. The rename path already refused a slug reserved to another
/// chat, so a replace only ever re-stamps this chat's own row.
pub fn retire_slug(
    conn: &Connection,
    project_id: &str,
    slug: &str,
    conversation_key: &str,
) -> Result<(), String> {
    if is_ordinal_token(slug) {
        return Ok(());
    }
    conn.execute(
        "INSERT INTO workspace_session_handle_aliases (project_id, slug, conversation_key, retired_at) \
         VALUES (?1, ?2, ?3, unixepoch()) \
         ON CONFLICT(project_id, slug) DO UPDATE SET \
            conversation_key = excluded.conversation_key, retired_at = excluded.retired_at",
        params![project_id, slug.trim(), conversation_key.trim()],
    )
    .map_err(|e| format!("retire name: {e}"))?;
    Ok(())
}

/// "Release old names" (TR6c): drop a chat's retired names in one
/// workspace (or every workspace when `project_id` is `None`). Returns
/// the slugs released.
pub fn release_aliases(
    conn: &Connection,
    project_id: Option<&str>,
    conversation_key: &str,
) -> Result<Vec<String>, String> {
    let key = conversation_key.trim();
    let mut released: Vec<String> = Vec::new();
    {
        // `project_id = ''` never matches a real row, so `''` means "any".
        let pid = project_id.unwrap_or("");
        let mut stmt = conn
            .prepare(
                "SELECT slug FROM workspace_session_handle_aliases \
                 WHERE conversation_key = ?1 AND (?2 = '' OR project_id = ?2) ORDER BY slug",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![key, pid], |r| r.get::<_, String>(0))
            .map_err(|e| e.to_string())?;
        for row in rows {
            released.push(row.map_err(|e| e.to_string())?);
        }
    }
    match project_id {
        Some(p) => conn.execute(
            "DELETE FROM workspace_session_handle_aliases \
             WHERE conversation_key = ?1 AND project_id = ?2",
            params![key, p],
        ),
        None => conn.execute(
            "DELETE FROM workspace_session_handle_aliases WHERE conversation_key = ?1",
            params![key],
        ),
    }
    .map_err(|e| format!("release names: {e}"))?;
    Ok(released)
}

/// TR6a: archiving a chat frees its current name (resolution skips
/// archived rows) and every retired name. Also drops any unclaimed mark.
pub fn free_names_on_archive(conn: &Connection, conversation_key: &str) -> Result<(), String> {
    release_aliases(conn, None, conversation_key)?;
    conn.execute(
        "DELETE FROM workspace_session_unclaimed_names WHERE conversation_key = ?1",
        params![conversation_key.trim()],
    )
    .map_err(|e| format!("free names on archive: {e}"))?;
    Ok(())
}

/// TR6b, called while the chat is still archived: if its current name
/// was taken meanwhile (by another chat's current name or a retired name
/// reserved to another chat), mark it unclaimed so it answers at its
/// ordinal. Returns a note for the restore response, or `None` when it
/// re-claims its name (or has none).
pub fn reclaim_on_restore(
    conn: &Connection,
    project_id: &str,
    conversation_key: &str,
) -> Result<Option<String>, String> {
    let key = conversation_key.trim();
    let name: Option<String> = conn
        .query_row(
            "SELECT custom_name FROM chat_session_names \
             WHERE session_id = ?1 AND TRIM(custom_name) != '' \
             ORDER BY updated_at DESC LIMIT 1",
            params![key],
            |r| r.get::<_, String>(0),
        )
        .map(Some)
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            other => Err(other.to_string()),
        })?;
    let Some(slug) = name
        .as_deref()
        .and_then(|n| slugify_custom_name(n).ok())
        .filter(|s| !is_ordinal_token(s))
    else {
        return Ok(None);
    };
    let held_by_current = find_conversation_by_slug(conn, project_id, &slug)
        .unwrap_or(None)
        .filter(|k| k != key);
    let held_by_alias = alias_owner(conn, project_id, &slug)?.filter(|k| k != key);
    if held_by_current.is_none() && held_by_alias.is_none() {
        conn.execute(
            "DELETE FROM workspace_session_unclaimed_names \
             WHERE project_id = ?1 AND conversation_key = ?2",
            params![project_id, key],
        )
        .map_err(|e| e.to_string())?;
        return Ok(None);
    }
    conn.execute(
        "INSERT OR IGNORE INTO workspace_session_unclaimed_names (project_id, conversation_key) \
         VALUES (?1, ?2)",
        params![project_id, key],
    )
    .map_err(|e| format!("mark unclaimed: {e}"))?;
    let ws = workspace_address_name(conn, project_id)?;
    let ordinal = allocate_ordinal(conn, project_id, key)?;
    Ok(Some(format!(
        "another chat took the name {ws}/{slug} while this one was archived; it keeps its Chats name and answers at {ws}/{ordinal} until you rename it"
    )))
}

/// Who owns a retired name, spelled for a refusal: `k2/3 (now k2/critic)`.
fn reserved_owner_phrase(conn: &Connection, project_id: &str, owner_key: &str) -> String {
    let ws = workspace_address_name(conn, project_id).unwrap_or_else(|_| "this workspace".into());
    let ordinal = get(conn, project_id, owner_key).ok().flatten().map(|r| r.ordinal);
    let current = current_handle_for(conn, project_id, owner_key).ok().flatten();
    match (ordinal, current) {
        (Some(o), Some(c)) if c != o.to_string() => format!("{ws}/{o} (now {ws}/{c})"),
        (Some(o), _) => format!("{ws}/{o}"),
        (None, Some(c)) => format!("{ws}/{c}"),
        (None, None) => format!("another chat in {ws}"),
    }
}

/// Refusal text when `slug` is a retired name reserved to `owner_key`.
pub fn reserved_name_hint(
    conn: &Connection,
    project_id: &str,
    slug: &str,
    owner_key: &str,
) -> String {
    format!(
        "'{slug}' was the name of {}. Old names stay reserved to their chat. Pick another name, or free it with Release old names on that chat (Chats, right-click).",
        reserved_owner_phrase(conn, project_id, owner_key)
    )
}

/// What a delivery address resolved to (Thread routes, `k2 msg`,
/// `k2 talk`, inject).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryTarget {
    pub conversation_key: String,
    /// The handle the conversation answers at now (`reviewer`, or `3`).
    pub current_handle: String,
}

/// Resolve `ws/<token>` for **delivery**. Old addresses keep working:
///
/// 1. All digits: the ordinal, always (TR2), even after a rename.
/// 2. A current name.
/// 3. A retired name: the chat that had it (reserved, so never a stranger).
/// 4. Else an error that lists the workspace's sidecars.
///
/// [`resolve_handle`] stays strict for naming jobs (`k2 sidecar new`,
/// `stop`, rename uniqueness).
pub fn resolve_for_delivery(
    conn: &Connection,
    project_id: &str,
    token: &str,
) -> Result<DeliveryTarget, String> {
    let token = token.trim();
    if token.is_empty() {
        return Err("empty sidecar handle".to_string());
    }
    if token.contains('/') || token.contains(':') {
        return Err(format!(
            "invalid sidecar handle '{token}' — use workspace/handle with a single slash"
        ));
    }
    let key = if is_ordinal_token(token) {
        let ordinal = token
            .parse::<u32>()
            .map_err(|_| format!("unknown sidecar handle '{token}' in this workspace"))?;
        if ordinal == 0 {
            return Err("sidecar ordinals start at 1".to_string());
        }
        match get_by_ordinal(conn, project_id, ordinal)? {
            Some(row) => row.conversation_key,
            None => return Err(unknown_handle_hint(conn, project_id, token)),
        }
    } else if let Some(key) = find_conversation_by_slug(conn, project_id, token)? {
        key
    } else if let Some(key) = alias_owner(conn, project_id, token)? {
        key
    } else {
        return Err(unknown_handle_hint(conn, project_id, token));
    };
    let current_handle = current_handle_for(conn, project_id, &key)?
        .unwrap_or_else(|| token.to_string());
    Ok(DeliveryTarget {
        conversation_key: key,
        current_handle,
    })
}

/// Unknown-handle error listing up to eight of the workspace's sidecars.
fn unknown_handle_hint(conn: &Connection, project_id: &str, token: &str) -> String {
    let ws = workspace_address_name(conn, project_id).unwrap_or_default();
    let mut known: Vec<String> = Vec::new();
    if let Ok(mut stmt) = conn.prepare(
        "SELECT conversation_key FROM workspace_session_handles \
         WHERE project_id = ?1 ORDER BY ordinal DESC LIMIT 8",
    ) {
        if let Ok(rows) = stmt.query_map(params![project_id], |r| r.get::<_, String>(0)) {
            for key in rows.flatten() {
                if let Ok(Some(h)) = current_handle_for(conn, project_id, &key) {
                    known.push(format_address(&ws, Some(&h)));
                }
            }
        }
    }
    if known.is_empty() {
        format!("unknown sidecar handle '{token}' in this workspace")
    } else {
        format!(
            "unknown sidecar handle '{token}' in this workspace (sidecars: {})",
            known.join(", ")
        )
    }
}

/// Resolve `handle` (`1` / `reviewer`) to a conversation_key — the
/// **strict** lookup for naming jobs. A current name match wins; an
/// all-digit token is only an ordinal (TR2). After a rename the old
/// ordinal fails loud here; delivery uses [`resolve_for_delivery`].
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

    if !is_ordinal_token(handle) {
        if let Some(key) = find_conversation_by_slug(conn, project_id, handle)? {
            return Ok(key);
        }
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

fn slug_matches(name: &str, slug: &str) -> bool {
    slugify_custom_name(name).ok().as_deref() == Some(slug)
}

fn push_unique(matches: &mut Vec<String>, key: String) {
    if !matches.iter().any(|m| m == &key) {
        matches.push(key);
    }
}

/// Every `chat_session_names` row whose slug matches, scoped to this
/// workspace. Disk-only chats (named in Chats, never a tab/handle row)
/// still count: if we cannot place the row in a *different* workspace,
/// treat it as in this one (fail loud).
fn collect_named_slug_matches(
    conn: &Connection,
    project_id: &str,
    slug: &str,
    matches: &mut Vec<String>,
) -> Result<(), String> {
    // Archived chats free their name (TR6a).
    let mut stmt = conn
        .prepare(
            "SELECT session_id, custom_name FROM chat_session_names \
             WHERE TRIM(custom_name) != '' AND archived = 0",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .map_err(|e| e.to_string())?;
    for row in rows {
        let (sid, name) = row.map_err(|e| e.to_string())?;
        if !slug_matches(&name, slug) {
            continue;
        }
        if name_is_unclaimed(conn, project_id, &sid)? {
            continue;
        }
        match project_id_for_session_id(conn, &sid)? {
            Some(pid) if pid != project_id => {}
            _ => push_unique(matches, sid),
        }
    }
    Ok(())
}

fn find_conversation_by_slug(
    conn: &Connection,
    project_id: &str,
    slug: &str,
) -> Result<Option<String>, String> {
    // TR2: digits are ordinals, never names.
    if is_ordinal_token(slug) {
        return Ok(None);
    }
    let mut matches: Vec<String> = Vec::new();

    collect_named_slug_matches(conn, project_id, slug, &mut matches)?;

    // Tab / API extra sessions (provider session_id on the tab row).
    // Kept so pane-keyed names still resolve if they were missed above.
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
            if address_slug_for(conn, Some(project_id), sid)?.as_deref() == Some(slug) {
                push_unique(&mut matches, sid.to_string());
            }
        } else if address_slug_for(conn, Some(project_id), &pane)?.as_deref() == Some(slug) {
            push_unique(&mut matches, pane);
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
        if address_slug_for(conn, Some(project_id), &key)?.as_deref() == Some(slug) {
            push_unique(&mut matches, key);
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

/// Refuse a slug that any other `chat_session_names` row already uses.
/// Used when the session cannot be placed in a workspace (disk-only).
pub fn ensure_slug_unique_among_names(
    conn: &Connection,
    session_id: &str,
    slug: &str,
) -> Result<(), String> {
    let mut stmt = conn
        .prepare(
            "SELECT session_id, custom_name FROM chat_session_names \
             WHERE TRIM(custom_name) != ''",
        )
        .map_err(|e| e.to_string())?;
    if is_ordinal_token(slug) {
        // TR2: an all-digit name is display only; it never claims an address.
        return Ok(());
    }
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .map_err(|e| e.to_string())?;
    for row in rows {
        let (sid, name) = row.map_err(|e| e.to_string())?;
        if sid == session_id {
            continue;
        }
        if slug_matches(&name, slug) {
            return Err(format!(
                "chat name slug '{slug}' is already used by another session in this workspace"
            ));
        }
    }
    Ok(())
}

/// Fail the second rename in a workspace that would share a slug, or
/// take a retired name reserved to another chat (Q2).
pub fn ensure_slug_unique_in_workspace(
    conn: &Connection,
    project_id: &str,
    session_id: &str,
    slug: &str,
) -> Result<(), String> {
    if is_ordinal_token(slug) {
        // TR2: an all-digit name is display only; it never claims an address.
        return Ok(());
    }
    if let Some(existing) = find_conversation_by_slug(conn, project_id, slug)? {
        if existing != session_id {
            return Err(format!(
                "chat name slug '{slug}' is already used by another session in this workspace"
            ));
        }
    }
    if let Some(owner) = alias_owner(conn, project_id, slug)? {
        if owner != session_id.trim() {
            return Err(reserved_name_hint(conn, project_id, slug, &owner));
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

/// Workspace address token: `projects.handle` (D7). Falls back to slugging
/// `projects.name` or the folder basename when handle is not yet minted
/// (unmigrated test rows). Never returns a display string with spaces.
pub fn workspace_address_name(conn: &Connection, project_id: &str) -> Result<String, String> {
    let (handle, name, path): (Option<String>, String, String) = conn
        .query_row(
            "SELECT handle, name, path FROM projects WHERE id = ?1",
            params![project_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .map_err(|_| format!("workspace not found: {project_id}"))?;
    if let Some(h) = handle {
        let h = h.trim();
        if !h.is_empty() {
            return Ok(h.to_string());
        }
    }
    let name = name.trim();
    if !name.is_empty() {
        if let Ok(slug) = slugify_address_token(name) {
            return Ok(slug);
        }
    }
    let base = std::path::Path::new(&path)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let base = base.trim();
    if base.is_empty() {
        return Err("workspace has no addressable name".to_string());
    }
    slugify_address_token(base).or_else(|_| Ok(base.to_ascii_lowercase()))
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
    if let Some(sid) = provider_session_id.map(str::trim).filter(|s| !s.is_empty()) {
        if conversation_is_canonical(conn, project_id, sid) {
            return Ok(None);
        }
    }
    let key = conversation_key_for(provider_session_id, pane_or_tab_key);
    if conversation_is_canonical(conn, project_id, &key) {
        return Ok(None);
    }
    if key.is_empty() {
        return Err("ensure_sidecar_handle: empty conversation_key".to_string());
    }
    if let Some(sid) = provider_session_id.map(str::trim).filter(|s| !s.is_empty()) {
        let pane = normalize_pane_key(pane_or_tab_key);
        if pane != sid {
            rekey_conversation(conn, project_id, pane, sid)?;
        }
    }
    // RC4: every sidecar gets a permanent ordinal at first register,
    // named or not, so a later rename always leaves `ws/<n>` behind.
    allocate_ordinal(conn, project_id, &key)?;
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
        // Live sidecar spellings must stay (D4 — do not change this fn).
        assert_eq!(slugify_custom_name("scout_v3").expect("ok"), "scout_v3");
        assert_eq!(
            slugify_custom_name("K2 - Marketing").expect("ok"),
            "k2---marketing"
        );
    }

    #[test]
    fn slugify_address_token_workspace_rules() {
        assert_eq!(slugify_address_token("sales").expect("ok"), "sales");
        assert_eq!(slugify_address_token("Sales").expect("ok"), "sales");
        assert_eq!(slugify_address_token("Sales Team").expect("ok"), "sales-team");
        assert_eq!(
            slugify_address_token("  Code Review  ").expect("ok"),
            "code-review"
        );
        assert_eq!(slugify_address_token("QA Bot").expect("ok"), "qa-bot");
        assert_eq!(
            slugify_address_token("K2 - Marketing Manager").expect("ok"),
            "k2-marketing-manager"
        );
        assert_eq!(slugify_address_token("scout_v3").expect("ok"), "scout-v3");
        slugify_address_token("sales/1").expect_err("slash");
        slugify_address_token("name::host").expect_err("colon");
        slugify_address_token("   ").expect_err("empty");
        assert!(address_tokens_match("Sales Team", "sales-team"));
        assert!(address_tokens_match("sales team", "sales-team"));
        assert!(
            !address_tokens_match("sales", "sales-team"),
            "D20: leftover pre-rename token must not slug-match the new handle"
        );
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
    fn canonical_workspace_session_is_not_a_sidecar_handle() {
        let project_id = seed_project("postal-bot");
        let grok = "7b2ae8f7-547b-42f3-8284-527742a36cc0";
        let claude_tab = "ab42f7df-0000-4000-8000-000000000001";
        {
            let dbh = conn();
            let c = dbh.lock();
            crate::db::schema::WorkspaceSession::upsert(
                &c,
                &format!("ws-row-{project_id}"),
                &project_id,
                None,
                Some(grok),
                "grok",
                "system",
                "running",
            )
            .expect("stamp canonical");
            assert!(conversation_is_canonical(&c, &project_id, grok));
            assert!(!conversation_is_canonical(&c, &project_id, claude_tab));
            let skipped = ensure_sidecar_handle(
                &c,
                &project_id,
                "tab-grok-pane",
                Some("grok"),
                Some(grok),
                "grok-pane",
            )
            .expect("ensure");
            assert!(
                skipped.is_none(),
                "canonical conversation must not mint ws/N, got {skipped:?}"
            );
            let extra = ensure_sidecar_handle(
                &c,
                &project_id,
                "tab-claude-pane",
                Some("claude"),
                Some(claude_tab),
                "claude-pane",
            )
            .expect("extra");
            assert_eq!(extra.as_deref(), Some("1"));
        }
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
    fn second_rename_collides_with_disk_only_chat() {
        let project_id = seed_project("sales-disk");
        let a = format!("sid-disk-a-{}", uuid::Uuid::new_v4());
        let b = format!("sid-disk-b-{}", uuid::Uuid::new_v4());
        // A exists only as a Chats custom_name — no tab, no handle row.
        insert_custom_name(&a, "Disk Reviewer");
        insert_tab(&project_id, "pb", Some(&b), "claude");
        let dbh = conn();
        let c = dbh.lock();
        crate::workspace_session_handles::ensure_slug_unique_in_workspace(
            &c, &project_id, &b, "disk-reviewer",
        )
        .expect_err("disk-only custom_name must block a second slug");
        crate::workspace_session_handles::ensure_slug_unique_among_names(&c, &b, "disk-reviewer")
            .expect_err("unplaced custom_name must also fail loud");
    }

    fn project_path_of(project_id: &str) -> String {
        let dbh = conn();
        let c = dbh.lock();
        c.query_row(
            "SELECT path FROM projects WHERE id = ?1",
            params![project_id],
            |r| r.get::<_, String>(0),
        )
        .expect("project path")
    }

    fn set_handle(project_id: &str, handle: &str) {
        let dbh = conn();
        let c = dbh.lock();
        c.execute(
            "UPDATE projects SET handle = ?2 WHERE id = ?1",
            params![project_id, handle],
        )
        .expect("set handle");
    }

    /// Seed a workspace with handle `ws` and one Claude sidecar at
    /// ordinal 1. Returns (project_id, path, conversation id).
    fn seed_sidecar(label: &str) -> (String, String, String) {
        let project_id = seed_project(label);
        set_handle(&project_id, &format!("ws-{}", &project_id[..8]));
        let sid = format!("sid-{label}-{}", uuid::Uuid::new_v4());
        insert_tab(&project_id, &format!("pane-{label}"), Some(&sid), "claude");
        let dbh = conn();
        let c = dbh.lock();
        let n = allocate_ordinal(&c, &project_id, &sid).expect("ordinal");
        assert_eq!(n, 1);
        drop(c);
        let path = project_path_of(&project_id);
        (project_id, path, sid)
    }

    fn rename(sid: &str, name: &str, path: &str) -> crate::chat_history::RenameOutcome {
        crate::chat_history::rename_session_scoped("claude", sid, name, Some(path))
            .unwrap_or_else(|e| panic!("rename {sid} to {name:?}: {e}"))
    }

    fn deliver(project_id: &str, token: &str) -> DeliveryTarget {
        let dbh = conn();
        let c = dbh.lock();
        resolve_for_delivery(&c, project_id, token)
            .unwrap_or_else(|e| panic!("deliver {token}: {e}"))
    }

    #[test]
    fn delivery_keeps_old_ordinal_and_old_names_after_renames() {
        let (pid, path, c_sid) = seed_sidecar("dlv");
        let ws = format!("ws-{}", &pid[..8]);
        let out = rename(&c_sid, "Reviewer", &path);
        assert_eq!(out.previous_address.as_deref(), Some(format!("{ws}/1").as_str()));
        assert_eq!(out.address.as_deref(), Some(format!("{ws}/reviewer").as_str()));
        assert!(out.address_changed());

        let t = deliver(&pid, "1");
        assert_eq!(t.conversation_key, c_sid);
        assert_eq!(t.current_handle, "reviewer");
        assert_eq!(deliver(&pid, "reviewer").conversation_key, c_sid);

        rename(&c_sid, "Critic", &path);
        let t = deliver(&pid, "reviewer");
        assert_eq!(t.conversation_key, c_sid, "a retired name stays an alias");
        assert_eq!(t.current_handle, "critic");
        assert_eq!(deliver(&pid, "1").current_handle, "critic");
        assert_eq!(deliver(&pid, "critic").conversation_key, c_sid);

        // Strict lookups stay strict (naming jobs).
        let dbh = conn();
        let c = dbh.lock();
        resolve_handle(&c, &pid, "reviewer").expect_err("strict: retired name is not a current name");
        resolve_handle(&c, &pid, "1").expect_err("strict: replaced ordinal still fails");
        let past = past_handles_for(&c, &pid, &c_sid).expect("past");
        assert_eq!(past, vec!["1".to_string(), "reviewer".to_string()]);
        assert_eq!(
            current_address_for(&c, &pid, &c_sid).expect("addr").as_deref(),
            Some(format!("{ws}/critic").as_str())
        );
        resolve_for_delivery(&c, &pid, "nobody").expect_err("unknown stays an error");
    }

    #[test]
    fn retired_name_is_reserved_until_released_and_owner_may_take_it_back() {
        let (pid, path, c_sid) = seed_sidecar("rsv");
        let d_sid = format!("sid-rsv-d-{}", uuid::Uuid::new_v4());
        insert_tab(&pid, "pane-rsv-d", Some(&d_sid), "claude");
        {
            let dbh = conn();
            let c = dbh.lock();
            allocate_ordinal(&c, &pid, &d_sid).expect("d ordinal");
        }
        rename(&c_sid, "Reviewer", &path);
        rename(&c_sid, "Critic", &path);
        let err = crate::chat_history::rename_session_scoped("claude", &d_sid, "Reviewer", Some(&path))
            .expect_err("another chat may not take a reserved name");
        assert!(err.contains("was the name of"), "{err}");
        assert!(err.contains("/1 (now"), "names the owner: {err}");
        assert!(err.contains("Release old names"), "{err}");

        // The original chat may take it back; it is then its current name.
        rename(&c_sid, "Reviewer", &path);
        assert_eq!(deliver(&pid, "reviewer").current_handle, "reviewer");
        assert_eq!(deliver(&pid, "critic").conversation_key, c_sid);

        // Release old names frees `critic` for D.
        {
            let dbh = conn();
            let c = dbh.lock();
            let released = release_aliases(&c, Some(&pid), &c_sid).expect("release");
            assert_eq!(released, vec!["critic".to_string()]);
        }
        rename(&d_sid, "Critic", &path);
        assert_eq!(deliver(&pid, "critic").conversation_key, d_sid);
    }

    #[test]
    fn archive_frees_current_and_retired_names() {
        // Archived rows show in every chat list: serialize with the chat
        // list tests (they share this lock and the test DB).
        let _home = crate::themes::HOME_LOCK.lock();
        let (pid, path, c_sid) = seed_sidecar("arc");
        let d_sid = format!("sid-arc-d-{}", uuid::Uuid::new_v4());
        insert_tab(&pid, "pane-arc-d", Some(&d_sid), "claude");
        rename(&c_sid, "Reviewer", &path);
        rename(&c_sid, "Critic", &path);
        crate::chat_user_archive::set_archived_flags(
            "claude",
            &c_sid,
            true,
            Some(1),
            Some(&path),
            Some("t"),
            Some(1),
            None,
        )
        .expect("archive");
        rename(&d_sid, "Reviewer", &path);
        assert_eq!(deliver(&pid, "reviewer").conversation_key, d_sid);
        rename(&d_sid, "Critic", &path);
        assert_eq!(deliver(&pid, "critic").conversation_key, d_sid);
        // C still answers at its permanent ordinal.
        let t = deliver(&pid, "1");
        assert_eq!(t.conversation_key, c_sid);
        assert_eq!(t.current_handle, "1", "an archived chat's name is not an address");
        // Leave no archived row behind: the shared test DB's chat list
        // tests expect a clean archive.
        {
            let dbh = conn();
            let c = dbh.lock();
            reclaim_on_restore(&c, &pid, &c_sid).expect("reclaim");
        }
        crate::chat_user_archive::set_archived_flags("claude", &c_sid, false, None, None, None, None, None)
            .expect("restore flags");
        assert_eq!(deliver(&pid, "critic").conversation_key, d_sid, "D keeps the name after C's restore");
    }

    #[test]
    fn restore_marks_a_taken_name_unclaimed() {
        let _home = crate::themes::HOME_LOCK.lock();
        let (pid, path, c_sid) = seed_sidecar("rst");
        let d_sid = format!("sid-rst-d-{}", uuid::Uuid::new_v4());
        insert_tab(&pid, "pane-rst-d", Some(&d_sid), "claude");
        rename(&c_sid, "Reviewer", &path);
        crate::chat_user_archive::set_archived_flags(
            "claude", &c_sid, true, Some(1), Some(&path), Some("t"), Some(1), None,
        )
        .expect("archive");
        rename(&d_sid, "Reviewer", &path);
        let note = {
            let dbh = conn();
            let c = dbh.lock();
            reclaim_on_restore(&c, &pid, &c_sid).expect("reclaim")
        };
        let note = note.expect("taken name must produce a note");
        assert!(note.contains("/1"), "{note}");
        crate::chat_user_archive::set_archived_flags("claude", &c_sid, false, None, None, None, None, None)
            .expect("restore flags");
        assert_eq!(deliver(&pid, "reviewer").conversation_key, d_sid, "no ambiguity after restore");
        assert_eq!(deliver(&pid, "1").current_handle, "1");
        // A rename re-claims.
        rename(&c_sid, "Scout", &path);
        assert_eq!(deliver(&pid, "1").current_handle, "scout");
    }

    #[test]
    fn all_digit_token_is_always_an_ordinal() {
        let (pid, path, c_sid) = seed_sidecar("dig");
        // Make room for ordinals 2 and 3 so C is not the only row.
        let d_sid = format!("sid-dig-d-{}", uuid::Uuid::new_v4());
        insert_tab(&pid, "pane-dig-d", Some(&d_sid), "claude");
        {
            let dbh = conn();
            let c = dbh.lock();
            assert_eq!(allocate_ordinal(&c, &pid, &d_sid).expect("d"), 2);
        }
        // D is named "1": a display name only.
        let out = rename(&d_sid, "1", &path);
        assert!(!out.address_changed(), "an all-digit name never moves the address: {out:?}");
        assert_eq!(deliver(&pid, "1").conversation_key, c_sid, "ws/1 stays C's ordinal");
        assert_eq!(deliver(&pid, "2").conversation_key, d_sid);
        let dbh = conn();
        let c = dbh.lock();
        assert_eq!(resolve_handle(&c, &pid, "1").expect("strict ordinal"), c_sid);
        assert_eq!(
            handle_for_session(&c, &pid, &d_sid, Some(&d_sid)).expect("h"),
            "2"
        );
    }

    #[test]
    fn named_before_first_register_still_gets_an_ordinal() {
        let project_id = seed_project("birth");
        set_handle(&project_id, &format!("ws-{}", &project_id[..8]));
        let path = project_path_of(&project_id);
        let sid = format!("sid-birth-{}", uuid::Uuid::new_v4());
        insert_tab(&project_id, "pane-birth", Some(&sid), "claude");
        rename(&sid, "Scout", &path);
        {
            let dbh = conn();
            let c = dbh.lock();
            let h = ensure_sidecar_handle(&c, &project_id, "tab-pane-birth", Some("claude"), Some(&sid), "pane-birth")
                .expect("ensure");
            assert_eq!(h.as_deref(), Some("scout"));
            assert!(get(&c, &project_id, &sid).expect("get").is_some(), "RC4: ordinal row exists");
        }
        rename(&sid, "Scout2", &path);
        assert_eq!(deliver(&project_id, "1").conversation_key, sid);
        assert_eq!(deliver(&project_id, "scout").conversation_key, sid);
        assert_eq!(deliver(&project_id, "scout").current_handle, "scout2");
    }

    #[test]
    fn rekey_moves_retired_names_with_the_handle_row() {
        let project_id = seed_project("rekey");
        set_handle(&project_id, &format!("ws-{}", &project_id[..8]));
        let path = project_path_of(&project_id);
        let pane = format!("pane-rk-{}", uuid::Uuid::new_v4());
        insert_tab(&project_id, &pane, None, "codex");
        {
            let dbh = conn();
            let c = dbh.lock();
            allocate_ordinal(&c, &project_id, &pane).expect("pane ordinal");
        }
        // Pane-keyed names (Codex before adoption).
        crate::chat_history::rename_session_scoped("codex", &pane, "Draft", Some(&path)).expect("r1");
        crate::chat_history::rename_session_scoped("codex", &pane, "Final", Some(&path)).expect("r2");
        assert_eq!(deliver(&project_id, "draft").conversation_key, pane);
        let provider = format!("prov-{}", uuid::Uuid::new_v4());
        {
            let dbh = conn();
            let c = dbh.lock();
            rekey_conversation(&c, &project_id, &pane, &provider).expect("rekey");
            assert_eq!(alias_owner(&c, &project_id, "draft").expect("owner").as_deref(), Some(provider.as_str()));
        }
        assert_eq!(deliver(&project_id, "1").conversation_key, provider);
    }

    #[test]
    fn is_uuid_shape_strict() {
        assert!(is_uuid_shape("a1b2c3d4-e5f6-7890-abcd-ef1234567890"));
        assert!(!is_uuid_shape("sales"));
        assert!(!is_uuid_shape("a1b2c3d4-e5f6-7890-abcd-ef1234567890/x"));
    }
}
