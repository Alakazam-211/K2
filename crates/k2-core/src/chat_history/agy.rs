//! Antigravity (`agy`) session discovery, for `chat_history`.

use super::*;

/// `~/.gemini/antigravity-cli/conversations/<id>.db`, when that
/// conversation's file is there.
pub(super) fn storage_path(home: &Path, session_id: &str) -> Option<PathBuf> {
    let dir = home.join(".gemini").join("antigravity-cli");
    if !agy_session_file_exists_in(&dir, session_id) {
        return None;
    }
    Some(dir.join("conversations").join(format!("{session_id}.db")))
}

// ── Antigravity (`agy`) session discovery ──────────────────────────────
//
// What agy 1.5.14 keeps on disk (as agy 1.5.14 stores it):
//   - `~/.gemini/antigravity-cli/conversations/<id>.db`: one SQLite file
//     per conversation; the id is a v4 UUID and is what
//     `agy --conversation <id>` takes, from any folder.
//   - `~/.gemini/antigravity-cli/conversation_summaries.db` (WAL): table
//     `conversation_summaries`, one row per conversation with `title`,
//     `preview` (the first prompt), `step_count`, `last_modified_time`
//     (`2026-11-22 04:33:58.051064987+00:00`), `workspace_uris` (a JSON
//     array of percent-encoded `file://` URIs: the folder agy ran in, a
//     worktree being its own folder) and `parent_conversation_id` (empty
//     for a person's own conversation, set for subagents).
// Message bodies are protobuf blobs inside the per-conversation file; K2
// doesn't read them (no transcript, title from the summary row).

/// `~/.gemini/antigravity-cli`: agy's default app-data folder.
fn agy_cli_dir() -> Option<PathBuf> {
    crate::paths::home_dir().map(|h| h.join(".gemini").join("antigravity-cli"))
}

/// Is there an agy conversation file for this id? Ignores the folder:
/// agy resumes an id from any folder.
pub fn agy_session_file_exists(session_id: &str) -> bool {
    agy_cli_dir().is_some_and(|d| agy_session_file_exists_in(&d, session_id))
}

/// Inner, HOME-free core of [`agy_session_file_exists`].
pub(super) fn agy_session_file_exists_in(cli_dir: &Path, session_id: &str) -> bool {
    looks_like_uuid(session_id)
        && cli_dir.join("conversations").join(format!("{session_id}.db")).is_file()
}

/// A `file://` URI from agy's `workspace_uris` as a local path:
/// percent-decoded, and on Windows without the `/` before the drive.
pub(super) fn agy_uri_path(uri: &str) -> Option<String> {
    let rest = uri.strip_prefix("file://")?;
    let bytes = rest.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
            if let Some(b) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    let path = String::from_utf8(out).ok()?;
    let b = path.as_bytes();
    if cfg!(windows) && b.len() >= 3 && b[0] == b'/' && b[1].is_ascii_alphabetic() && b[2] == b':' {
        return Some(path[1..].to_string());
    }
    Some(path)
}

/// agy's `last_modified_time` (`2026-11-22 04:33:58.051064987+00:00`) in
/// unix ms; 0 when it doesn't parse.
fn agy_time_ms(s: &str) -> i64 {
    parse_rfc3339_to_ms(&s.trim().replacen(' ', "T", 1)).unwrap_or(0)
}

/// The most recent agy conversation for a project family (the folder or a
/// worktree under it): the `chat/detect-active` entry and the resume
/// adapter's newest-on-disk probe. No store / lock contention → `None`.
pub fn detect_agy_session(project_path: &str) -> Option<String> {
    let dir = agy_cli_dir()?;
    agy_sessions_in(&dir, Some(project_path)).into_iter().next().map(|s| s.session_id)
}

/// agy conversations for Chat history, newest first. A person's own
/// conversations only (no subagents), and only those whose conversation
/// file is still there (resuming a missing one would start a new chat).
/// `message_count` is agy's step count (a prompt and its answer are two).
/// No store → empty, never an error.
pub fn parse_agy_sessions(project_filter: Option<&str>) -> Result<Vec<ChatSession>, String> {
    let Some(dir) = agy_cli_dir() else {
        return Ok(vec![]);
    };
    Ok(agy_sessions_in(&dir, project_filter))
}

/// Inner, HOME-free core of [`parse_agy_sessions`] / [`detect_agy_session`].
pub(super) fn agy_sessions_in(cli_dir: &Path, project_filter: Option<&str>) -> Vec<ChatSession> {
    let Some(conn) = open_hermes_db_ro(&cli_dir.join("conversation_summaries.db")) else {
        return vec![];
    };
    let mut stmt = match conn.prepare(
        "SELECT conversation_id, title, preview, step_count, last_modified_time, workspace_uris \
         FROM conversation_summaries WHERE parent_conversation_id = '' \
         ORDER BY last_modified_time DESC",
    ) {
        Ok(s) => s,
        Err(_) => return vec![],
    };
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, i64>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, String>(5)?,
        ))
    });
    let Ok(rows) = rows else {
        return vec![];
    };
    let root = project_filter.map(resolve_root_project_path);
    let mut out: Vec<ChatSession> = Vec::new();
    for (id, title, preview, steps, modified, uris) in rows.flatten() {
        if !agy_session_file_exists_in(cli_dir, &id) {
            continue;
        }
        let paths: Vec<String> = serde_json::from_str::<Vec<String>>(&uris)
            .unwrap_or_default()
            .iter()
            .filter_map(|u| agy_uri_path(u))
            .collect();
        let project = match root {
            Some(r) => match paths.iter().find(|p| matches_project_family(p.trim_end_matches('/'), r)) {
                Some(p) => p.clone(),
                None => continue,
            },
            None => paths.first().cloned().unwrap_or_default(),
        };
        let raw = if title.trim().is_empty() { preview } else { title };
        let trimmed = raw.trim();
        let title = if trimmed.is_empty() {
            "Untitled".to_string()
        } else if trimmed.chars().count() > 80 {
            format!("{}...", trimmed.chars().take(77).collect::<String>())
        } else {
            trimmed.to_string()
        };
        out.push(ChatSession {
            session_id: id,
            origin_branch: extract_worktree_branch(&project),
            project,
            title,
            timestamp: agy_time_ms(&modified),
            provider: "agy".to_string(),
            message_count: steps.max(0) as usize,
            archived: false,
            archived_at: None,
            custom_name: None,
        });
    }
    // The SQL order is by text; a changed UTC offset would misorder it.
    out.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    out
}
