//! Clone-to pinned-chat identity capture + apply.
//!
//! Captures the source workspace's default-resume chat (`workspace_sessions`)
//! and named/pinned chat list (`chat_session_names`) into the bundle
//! manifest, then re-applies them on the destination after unpack +
//! project registration. Never migrates credentials.

use super::{ChatPinEntry, CloneManifest, PinnedChatIdentity};
use crate::chat_history::{
    claude_project_hash, claude_session_file_exists, newest_claude_session_on_disk,
    resolve_root_project_path,
};
use crate::db::schema::WorkspaceSession;
use rusqlite::Connection;
use std::path::Path;

/// Look up the source project's pinned (default-resume) chat identity
/// from `workspace_sessions`, matching the project by absolute path with
/// the same trailing-slash normalization as [`super::capture_settings`].
///
/// Returns `Ok(None)` when the path isn't a registered project, the
/// workspace has no session row, or `session_id` is empty/null.
pub fn capture_pinned_chat(
    conn: &Connection,
    project_path: &str,
) -> Result<Option<PinnedChatIdentity>, String> {
    let project_id = match lookup_project_id(conn, project_path)? {
        Some(id) => id,
        None => return Ok(None),
    };

    let mut stmt = conn
        .prepare(
            "SELECT session_id, harness FROM workspace_sessions \
             WHERE project_id = ?1 LIMIT 1",
        )
        .map_err(|e| format!("prepare pinned-chat query: {e}"))?;

    let row = stmt.query_row(rusqlite::params![project_id], |row| {
        let session_id: Option<String> = row.get(0)?;
        let harness: String = row.get(1)?;
        Ok((session_id, harness))
    });

    match row {
        Ok((Some(sid), harness)) if !sid.is_empty() => Ok(Some(PinnedChatIdentity {
            session_id: sid,
            harness: if harness.is_empty() {
                "claude".to_string()
            } else {
                harness
            },
        })),
        Ok(_) => Ok(None),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(format!("read workspace_sessions row: {e}")),
    }
}

/// Capture named and/or pinned chat entries for the given session ids
/// from `chat_session_names`. Rows with `pinned=0` and empty
/// `custom_name` are skipped — only meaningful identity travels.
pub fn capture_chat_pins(
    conn: &Connection,
    session_ids: &[String],
) -> Result<Vec<ChatPinEntry>, String> {
    if session_ids.is_empty() {
        return Ok(Vec::new());
    }

    // Build `IN (?,?,…)` placeholders. session_ids are UUID-shaped stems
    // from inventory; empty strings are filtered so we never match junk.
    let ids: Vec<&str> = session_ids
        .iter()
        .map(|s| s.as_str())
        .filter(|s| !s.is_empty())
        .collect();
    if ids.is_empty() {
        return Ok(Vec::new());
    }

    let placeholders: String = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let sql = format!(
        "SELECT provider, session_id, custom_name, pinned \
         FROM chat_session_names \
         WHERE session_id IN ({placeholders}) \
           AND (pinned = 1 OR custom_name != '')"
    );

    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| format!("prepare chat_pins query: {e}"))?;

    let rows = stmt
        .query_map(rusqlite::params_from_iter(ids.iter()), |row| {
            let pinned_i: i64 = row.get(3)?;
            Ok(ChatPinEntry {
                provider: row.get(0)?,
                session_id: row.get(1)?,
                custom_name: row.get::<_, String>(2).unwrap_or_default(),
                pinned: pinned_i != 0,
            })
        })
        .map_err(|e| format!("query chat_pins: {e}"))?;

    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| format!("read chat_pin row: {e}"))?);
    }
    Ok(out)
}

/// Apply pinned-chat identity + chat pins from a clone manifest onto a
/// freshly registered destination project. Best-effort: errors are
/// logged via the returned `Result` so the caller can ignore them
/// without failing the whole unpack.
///
/// `home` is the remote home dir sessions were unpacked under
/// (`dirs::home_dir()` in production, a temp dir in hermetic tests).
/// When `Some`, Claude session existence is checked under that home so
/// tests and home-override unpacks still pin correctly.
pub fn apply_clone_identity(
    conn: &Connection,
    project_id: &str,
    dest_path: &str,
    home: Option<&Path>,
    manifest: &CloneManifest,
) -> Result<(), String> {
    // ── 1. Resolve target session_id for workspace_sessions ──────────
    let harness = manifest
        .pinned_chat
        .as_ref()
        .map(|p| p.harness.as_str())
        .filter(|h| !h.is_empty())
        .unwrap_or("claude");

    let target_sid = resolve_target_session_id(manifest, dest_path, harness, home);

    if let Some(sid) = target_sid {
        let row_id = uuid::Uuid::new_v4().to_string();
        WorkspaceSession::upsert(
            conn,
            &row_id,
            project_id,
            None, // terminal_id — destination has no live PTY yet
            Some(&sid),
            harness,
            "system",
            "stopped",
        )
        .map_err(|e| format!("upsert workspace_sessions: {e}"))?;
    }

    // ── 2. Upsert chat_session_names for each captured pin ───────────
    for pin in &manifest.chat_pins {
        if pin.session_id.is_empty() || pin.provider.is_empty() {
            continue;
        }
        let pinned_val: i64 = if pin.pinned { 1 } else { 0 };
        conn.execute(
            "INSERT INTO chat_session_names (provider, session_id, custom_name, pinned, updated_at) \
             VALUES (?1, ?2, ?3, ?4, unixepoch()) \
             ON CONFLICT(provider, session_id) DO UPDATE SET \
               custom_name = ?3, pinned = ?4, updated_at = unixepoch()",
            rusqlite::params![
                pin.provider,
                pin.session_id,
                pin.custom_name,
                pinned_val
            ],
        )
        .map_err(|e| format!("upsert chat_session_names: {e}"))?;
    }

    Ok(())
}

/// Pick the session_id to stamp on the destination workspace:
/// 1. Manifest pin when the session file still exists on dest
/// 2. Else newest Claude session on disk (Claude harness only)
/// 3. Else None
fn resolve_target_session_id(
    manifest: &CloneManifest,
    dest_path: &str,
    harness: &str,
    home: Option<&Path>,
) -> Option<String> {
    if let Some(pin) = &manifest.pinned_chat {
        if !pin.session_id.is_empty()
            && session_exists_on_dest(&pin.session_id, dest_path, harness, home)
        {
            return Some(pin.session_id.clone());
        }
    }

    // Fall back to newest on-disk Claude session when harness is claude
    // (or the pin pointed at a missing file for a Claude workspace).
    if harness == "claude" {
        if let Some(h) = home {
            return newest_claude_session_under_home(dest_path, h);
        }
        return newest_claude_session_on_disk(dest_path);
    }

    None
}

fn session_exists_on_dest(
    session_id: &str,
    dest_path: &str,
    harness: &str,
    home: Option<&Path>,
) -> bool {
    match harness {
        "claude" => {
            if let Some(h) = home {
                claude_session_exists_under_home(session_id, dest_path, h)
            } else {
                claude_session_file_exists(session_id, dest_path)
            }
        }
        // Other harnesses: use public exists helpers when available.
        "grok" => crate::chat_history::grok_session_file_exists(session_id, dest_path),
        "cursor" => crate::chat_history::cursor_session_file_exists(session_id, dest_path),
        _ => false,
    }
}

/// Home-parameterized Claude session existence — mirrors
/// [`claude_session_file_exists`] but roots under `home` so hermetic
/// unpacks (tests + home_override) pin correctly.
fn claude_session_exists_under_home(session_id: &str, project_path: &str, home: &Path) -> bool {
    let project_hash = claude_project_hash(resolve_root_project_path(project_path));
    let projects_dir = home.join(".claude").join("projects");
    let Ok(entries) = std::fs::read_dir(&projects_dir) else {
        return false;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name == project_hash || name.starts_with(&format!("{project_hash}-")) {
            if entry
                .path()
                .join(format!("{session_id}.jsonl"))
                .exists()
            {
                return true;
            }
        }
    }
    false
}

/// Home-parameterized newest Claude session — mirrors
/// [`newest_claude_session_on_disk`] under `home`.
fn newest_claude_session_under_home(project_path: &str, home: &Path) -> Option<String> {
    let project_hash = claude_project_hash(resolve_root_project_path(project_path));
    let projects_dir = home.join(".claude").join("projects");
    let entries = std::fs::read_dir(projects_dir).ok()?;
    let mut best: Option<(std::time::SystemTime, String)> = None;

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !(name == project_hash || name.starts_with(&format!("{project_hash}-"))) {
            continue;
        }
        let Ok(session_files) = std::fs::read_dir(entry.path()) else {
            continue;
        };
        for sf in session_files.flatten() {
            let path = sf.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let Ok(meta) = sf.metadata() else {
                continue;
            };
            // Skip zero-byte pre-allocated files (same as chat_history).
            if meta.len() == 0 {
                continue;
            }
            let Ok(mtime) = meta.modified() else {
                continue;
            };
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if stem.is_empty() {
                continue;
            }
            let better = match &best {
                None => true,
                Some((prev, _)) => mtime > *prev,
            };
            if better {
                best = Some((mtime, stem.to_string()));
            }
        }
    }
    best.map(|(_, id)| id)
}

/// Path-normalized project id lookup (exact / trim slash / with slash).
fn lookup_project_id(conn: &Connection, project_path: &str) -> Result<Option<String>, String> {
    let trimmed = project_path.trim_end_matches('/');
    let with_sep = format!("{trimmed}/");

    let mut stmt = conn
        .prepare(
            "SELECT id FROM projects \
             WHERE path = ?1 OR path = ?2 OR path = ?3 \
             LIMIT 1",
        )
        .map_err(|e| format!("prepare project id lookup: {e}"))?;

    let row = stmt.query_row(
        rusqlite::params![project_path, trimmed, with_sep],
        |row| row.get::<_, String>(0),
    );

    match row {
        Ok(id) => Ok(Some(id)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(format!("lookup project id: {e}")),
    }
}

/// Collect session UUIDs (file stems) from inventory entries of class
/// Session (and Provider when present). Used by the daemon pack path
/// to scope `capture_chat_pins`.
pub fn session_ids_from_entries(
    entries: impl IntoIterator<Item = (super::DestinationClass, String)>,
) -> Vec<String> {
    let mut ids = Vec::new();
    for (class, rel_path) in entries {
        match class {
            super::DestinationClass::Session | super::DestinationClass::Provider => {
                // rel_path is like `<slug>/<uuid>.jsonl` or
                // `<slug>-branch/<uuid>.jsonl` — stem is the session id.
                if let Some(stem) = Path::new(&rel_path)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .filter(|s| !s.is_empty())
                {
                    if !ids.iter().any(|e| e == stem) {
                        ids.push(stem.to_string());
                    }
                }
            }
            _ => {}
        }
    }
    ids
}
