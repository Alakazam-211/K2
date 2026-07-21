//! Multi-provider session locate for Clone-to (C1–C3).
//!
//! Collects provider-owned chat sessions for a workspace family and
//! stages them under the `providers/` archive prefix
//! ([`DestinationClass::Provider`]). Claude remains under
//! `sessions/` / `memory/` (unchanged).
//!
//! ## Hard rule — NO credentials
//! Never enumerate:
//! - `~/.claude/.credentials.json`
//! - `~/.grok/auth.json` / any `*auth*`
//! - API keys, OAuth blobs, IDE account DBs
//! - Whole Hermes `state.db` wholesale
//!
//! Destination machines have their own subscriptions. Sessions/history only.
//!
//! ## Archive layout (relative under `providers/`)
//! ```text
//! gemini/tmp/<slug>/chats/*.jsonl
//! pi/agent/sessions/<slug>/<file>.jsonl
//! codex/sessions/YYYY/MM/DD/rollout-*.jsonl
//! grok/sessions/<src-pct-encoded-cwd>/<uuid>/{summary.json,chat_history.jsonl,...}
//! cursor/chats/<md5(src)>/<uuid>/store.db
//! hermes/export.json          # JSON export of matching session+message rows
//! ```
//! Unpack re-roots onto `home/.<rest>` and re-keys Cursor MD5 / Grok
//! percent-encoded cwd dirs for the dest path; Hermes merges rows.

use super::{CloneOptions, DestinationClass, InventoryEntry};
use crate::chat_history::{matches_project_family, md5_hex, resolve_root_project_path};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

/// Collect every Big-7 (non-Claude) provider session artifact for
/// `project_path` into `entries` as [`DestinationClass::Provider`].
pub fn collect_provider_sessions(
    home: &Path,
    project_path: &str,
    opts: &CloneOptions,
    entries: &mut Vec<InventoryEntry>,
) {
    let root = resolve_root_project_path(project_path).to_string();
    locate_gemini(home, &root, opts, entries);
    locate_pi(home, &root, opts, entries);
    locate_codex(home, &root, opts, entries);
    locate_grok(home, &root, opts, entries);
    locate_cursor(home, &root, opts, entries);
    locate_hermes(home, &root, opts, entries);
}

fn push_provider(entries: &mut Vec<InventoryEntry>, abs: PathBuf, rel: String) {
    if !abs.is_file() {
        return;
    }
    entries.push(InventoryEntry {
        abs_path: abs,
        rel_path: rel,
        class: DestinationClass::Provider,
    });
}

/// Keep only the newest-mtime file when `!include_all_history`.
fn maybe_newest_only(
    mut found: Vec<(PathBuf, String, std::time::SystemTime)>,
    include_all: bool,
) -> Vec<(PathBuf, String)> {
    if found.is_empty() {
        return vec![];
    }
    if include_all {
        return found.into_iter().map(|(a, r, _)| (a, r)).collect();
    }
    found.sort_by(|a, b| b.2.cmp(&a.2));
    let (a, r, _) = found.into_iter().next().unwrap();
    vec![(a, r)]
}

fn file_mtime(path: &Path) -> std::time::SystemTime {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .unwrap_or(std::time::UNIX_EPOCH)
}

// ── Gemini ────────────────────────────────────────────────────────────

/// Locate Gemini sessions whose `projects.json` cwd is in the workspace
/// family. Rel: `gemini/tmp/<slug>/chats/<file>`.
fn locate_gemini(
    home: &Path,
    root: &str,
    opts: &CloneOptions,
    entries: &mut Vec<InventoryEntry>,
) {
    let projects_json = home.join(".gemini").join("projects.json");
    let tmp_dir = home.join(".gemini").join("tmp");
    if !tmp_dir.is_dir() {
        return;
    }
    let content = match fs::read_to_string(&projects_json) {
        Ok(c) => c,
        Err(_) => return,
    };
    let parsed: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return,
    };
    let projects_obj = match parsed.get("projects").and_then(|v| v.as_object()) {
        Some(o) => o,
        None => return,
    };

    let mut found: Vec<(PathBuf, String, std::time::SystemTime)> = Vec::new();
    for (cwd, slug_v) in projects_obj {
        if !matches_project_family(cwd, root) {
            continue;
        }
        let slug = match slug_v.as_str() {
            Some(s) => s,
            None => continue,
        };
        let chats_dir = tmp_dir.join(slug).join("chats");
        let rd = match fs::read_dir(&chats_dir) {
            Ok(r) => r,
            Err(_) => continue,
        };
        for ent in rd.flatten() {
            let path = ent.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            if !path.is_file() {
                continue;
            }
            let rel = format!(
                "gemini/tmp/{}/chats/{}",
                slug,
                path.file_name().unwrap_or_default().to_string_lossy()
            );
            found.push((path, rel, file_mtime(&ent.path())));
        }
    }
    for (abs, rel) in maybe_newest_only(found, opts.include_all_history) {
        push_provider(entries, abs, rel);
    }
}

// ── Pi ────────────────────────────────────────────────────────────────

/// Walk `~/.pi/agent/sessions/**/*.jsonl`, keep files whose line-1 `cwd`
/// matches the family. Rel preserves structure under `pi/agent/sessions/`.
fn locate_pi(
    home: &Path,
    root: &str,
    opts: &CloneOptions,
    entries: &mut Vec<InventoryEntry>,
) {
    let sessions_root = home.join(".pi").join("agent").join("sessions");
    if !sessions_root.is_dir() {
        return;
    }
    let mut found: Vec<(PathBuf, String, std::time::SystemTime)> = Vec::new();
    let slug_dirs = match fs::read_dir(&sessions_root) {
        Ok(r) => r,
        Err(_) => return,
    };
    for slug_entry in slug_dirs.flatten() {
        let slug_path = slug_entry.path();
        if !slug_path.is_dir() {
            continue;
        }
        let slug_name = slug_entry.file_name().to_string_lossy().to_string();
        let files = match fs::read_dir(&slug_path) {
            Ok(r) => r,
            Err(_) => continue,
        };
        for f_entry in files.flatten() {
            let path = f_entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            if !path.is_file() {
                continue;
            }
            // Peek line 1 for cwd.
            let cwd = match peek_json_field(&path, &["cwd"]) {
                Some(c) => c,
                None => continue,
            };
            if !matches_project_family(&cwd, root) {
                continue;
            }
            let file_name = path.file_name().unwrap_or_default().to_string_lossy();
            let rel = format!("pi/agent/sessions/{slug_name}/{file_name}");
            found.push((path, rel, file_mtime(&f_entry.path())));
        }
    }
    for (abs, rel) in maybe_newest_only(found, opts.include_all_history) {
        push_provider(entries, abs, rel);
    }
}

// ── Codex ─────────────────────────────────────────────────────────────

/// Walk `~/.codex/sessions/**/rollout-*.jsonl`, filter by `payload.cwd`.
/// Does **not** copy global `history.jsonl`.
fn locate_codex(
    home: &Path,
    root: &str,
    opts: &CloneOptions,
    entries: &mut Vec<InventoryEntry>,
) {
    let sessions_root = home.join(".codex").join("sessions");
    if !sessions_root.is_dir() {
        return;
    }
    let mut found: Vec<(PathBuf, String, std::time::SystemTime)> = Vec::new();
    walk_codex_rollouts(&sessions_root, &sessions_root, root, &mut found);
    for (abs, rel) in maybe_newest_only(found, opts.include_all_history) {
        push_provider(entries, abs, rel);
    }
}

fn walk_codex_rollouts(
    sessions_root: &Path,
    dir: &Path,
    root: &str,
    found: &mut Vec<(PathBuf, String, std::time::SystemTime)>,
) {
    let rd = match fs::read_dir(dir) {
        Ok(r) => r,
        Err(_) => return,
    };
    for ent in rd.flatten() {
        let path = ent.path();
        if path.is_dir() {
            walk_codex_rollouts(sessions_root, &path, root, found);
            continue;
        }
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if !name.starts_with("rollout-") || !name.ends_with(".jsonl") {
            continue;
        }
        if !path.is_file() {
            continue;
        }
        // payload.cwd from session_meta header
        let cwd = match peek_codex_cwd(&path) {
            Some(c) => c,
            None => continue,
        };
        if !matches_project_family(&cwd, root) {
            continue;
        }
        let rel = match path.strip_prefix(sessions_root) {
            Ok(r) => format!("codex/sessions/{}", r.to_string_lossy().replace('\\', "/")),
            Err(_) => continue,
        };
        found.push((path, rel, file_mtime(&ent.path())));
    }
}

fn peek_codex_cwd(path: &Path) -> Option<String> {
    let file = fs::File::open(path).ok()?;
    let mut reader = BufReader::new(file);
    let mut first_line = String::new();
    reader.read_line(&mut first_line).ok()?;
    let header: serde_json::Value = serde_json::from_str(first_line.trim()).ok()?;
    if header.get("type").and_then(|v| v.as_str()) != Some("session_meta") {
        return None;
    }
    header
        .get("payload")
        .and_then(|p| p.get("cwd"))
        .and_then(|v| v.as_str())
        .map(String::from)
}

// ── Grok ──────────────────────────────────────────────────────────────

/// Locate `~/.grok/sessions/<encoded-cwd>/<uuid>/` where summary.json
/// `info.cwd` matches family. Skip subagents, FTS sqlite, active_sessions,
/// auth.json, downloads.
fn locate_grok(
    home: &Path,
    root: &str,
    opts: &CloneOptions,
    entries: &mut Vec<InventoryEntry>,
) {
    let sessions_root = home.join(".grok").join("sessions");
    if !sessions_root.is_dir() {
        return;
    }
    // Group files by session dir so "newest only" keeps a whole session.
    let mut sessions: Vec<(PathBuf, String, std::time::SystemTime)> = Vec::new();
    let cwd_dirs = match fs::read_dir(&sessions_root) {
        Ok(r) => r,
        Err(_) => return,
    };
    for cwd_entry in cwd_dirs.flatten() {
        let cwd_path = cwd_entry.path();
        if !cwd_path.is_dir() {
            continue; // skip session_search.sqlite, locks, etc.
        }
        let cwd_dir_name = cwd_entry.file_name().to_string_lossy().to_string();
        // Never touch credential-adjacent dirs if they ever appear here.
        if cwd_dir_name.eq_ignore_ascii_case("auth")
            || cwd_dir_name.contains("auth")
            || cwd_dir_name == "downloads"
        {
            continue;
        }
        let session_dirs = match fs::read_dir(&cwd_path) {
            Ok(r) => r,
            Err(_) => continue,
        };
        for session_entry in session_dirs.flatten() {
            let session_dir = session_entry.path();
            if !session_dir.is_dir() {
                continue;
            }
            let uuid = session_entry.file_name().to_string_lossy().to_string();
            if !looks_like_uuid(&uuid) {
                continue;
            }
            let summary_path = session_dir.join("summary.json");
            if !summary_path.is_file() {
                continue;
            }
            let content = match fs::read_to_string(&summary_path) {
                Ok(c) => c,
                Err(_) => continue,
            };
            let parsed: serde_json::Value = match serde_json::from_str(&content) {
                Ok(v) => v,
                Err(_) => continue,
            };
            // Skip subagents.
            let kind = parsed
                .get("session_kind")
                .and_then(|v| v.as_str())
                .or_else(|| {
                    parsed
                        .get("info")
                        .and_then(|i| i.get("session_kind"))
                        .and_then(|v| v.as_str())
                });
            if kind == Some("subagent") {
                continue;
            }
            let cwd = match parsed
                .get("info")
                .and_then(|i| i.get("cwd"))
                .and_then(|v| v.as_str())
            {
                Some(c) => c,
                None => continue,
            };
            if !matches_project_family(cwd, root) {
                continue;
            }
            let mtime = file_mtime(&summary_path);
            sessions.push((session_dir, format!("grok/sessions/{cwd_dir_name}/{uuid}"), mtime));
        }
    }

    // Select session dirs (all or newest).
    let selected: Vec<(PathBuf, String)> = if opts.include_all_history {
        sessions.into_iter().map(|(d, r, _)| (d, r)).collect()
    } else if sessions.is_empty() {
        vec![]
    } else {
        let mut s = sessions;
        s.sort_by(|a, b| b.2.cmp(&a.2));
        let (d, r, _) = s.into_iter().next().unwrap();
        vec![(d, r)]
    };

    for (session_dir, rel_prefix) in selected {
        // Copy safe files only — skip nested subagents/, anything auth-like.
        let walker = match fs::read_dir(&session_dir) {
            Ok(r) => r,
            Err(_) => continue,
        };
        for ent in walker.flatten() {
            let path = ent.path();
            let name = ent.file_name().to_string_lossy().to_string();
            if name.eq_ignore_ascii_case("auth.json")
                || name.contains("auth")
                || name == "subagents"
                || name.ends_with(".sqlite")
                || name.ends_with(".db")
            {
                continue;
            }
            if path.is_dir() {
                // Shallow: only copy regular files at session root.
                // Nested dirs (subagents already skipped by name) ignored.
                continue;
            }
            if !path.is_file() {
                continue;
            }
            let rel = format!("{rel_prefix}/{name}");
            push_provider(entries, path, rel);
        }
    }
}

fn looks_like_uuid(name: &str) -> bool {
    let bytes = name.as_bytes();
    bytes.len() == 36
        && bytes.iter().enumerate().all(|(i, b)| match i {
            8 | 13 | 18 | 23 => *b == b'-',
            _ => b.is_ascii_hexdigit(),
        })
}

// ── Cursor ────────────────────────────────────────────────────────────

/// Locate `~/.cursor/chats/<md5(src_path)>/<uuid>/store.db`.
/// Never touches IDE Application Support account DBs.
fn locate_cursor(
    home: &Path,
    root: &str,
    opts: &CloneOptions,
    entries: &mut Vec<InventoryEntry>,
) {
    let chats_dir = home.join(".cursor").join("chats");
    if !chats_dir.is_dir() {
        return;
    }
    let root_hash = md5_hex(root.as_bytes());
    let hash_dir = chats_dir.join(&root_hash);
    if !hash_dir.is_dir() {
        return;
    }
    let mut found: Vec<(PathBuf, String, std::time::SystemTime)> = Vec::new();
    let chat_dirs = match fs::read_dir(&hash_dir) {
        Ok(r) => r,
        Err(_) => return,
    };
    for chat_entry in chat_dirs.flatten() {
        if !chat_entry.path().is_dir() {
            continue;
        }
        let uuid = chat_entry.file_name().to_string_lossy().to_string();
        let store_db = chat_entry.path().join("store.db");
        if !store_db.is_file() {
            continue;
        }
        let rel = format!("cursor/chats/{root_hash}/{uuid}/store.db");
        found.push((store_db, rel, file_mtime(&chat_entry.path().join("store.db"))));
    }
    for (abs, rel) in maybe_newest_only(found, opts.include_all_history) {
        push_provider(entries, abs, rel);
    }
}

// ── Hermes ────────────────────────────────────────────────────────────

/// RO-open `~/.hermes/state.db`, SELECT sessions (+ messages) for cwd
/// family, write `hermes/export.json` staging file, inventory that file.
/// Never ships the whole DB.
fn locate_hermes(
    home: &Path,
    root: &str,
    opts: &CloneOptions,
    entries: &mut Vec<InventoryEntry>,
) {
    let db_path = home.join(".hermes").join("state.db");
    if !db_path.is_file() {
        return;
    }
    let export = match export_hermes_sessions(&db_path, root, opts.include_all_history) {
        Some(v) if !v.is_empty() => v,
        _ => return,
    };
    // Stage under a process-private temp file the bundler can open.
    let staging = staging_hermes_path(home, root);
    if let Some(parent) = staging.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let bytes = match serde_json::to_vec_pretty(&export) {
        Ok(b) => b,
        Err(_) => return,
    };
    if fs::write(&staging, &bytes).is_err() {
        return;
    }
    push_provider(entries, staging, "hermes/export.json".to_string());
}

fn staging_hermes_path(home: &Path, root: &str) -> PathBuf {
    // Prefer a path under the hermetic home when tests override HOME so the
    // fixture TempDir cleans it up. Otherwise stage under the process temp
    // dir (never next to real state.db, never a credentials path).
    let hash = md5_hex(root.as_bytes());
    // Heuristic: if `home` is not the real user home, treat it as a test
    // sandbox and write under it; otherwise use OS temp.
    let use_home = dirs::home_dir().map(|h| h != home).unwrap_or(true);
    if use_home {
        home.join(".hermes")
            .join(format!(".k2-clone-export-{hash}.json"))
    } else {
        std::env::temp_dir().join(format!("k2-clone-hermes-export-{hash}.json"))
    }
}

/// Exported Hermes session payload (session row + messages).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct HermesExportedSession {
    pub id: String,
    pub source: String,
    pub parent_session_id: Option<String>,
    pub started_at: f64,
    pub ended_at: Option<f64>,
    pub end_reason: Option<String>,
    pub message_count: i64,
    pub title: Option<String>,
    pub cwd: Option<String>,
    pub git_branch: Option<String>,
    pub git_repo_root: Option<String>,
    pub archived: i64,
    pub messages: Vec<HermesExportedMessage>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct HermesExportedMessage {
    pub role: String,
    pub content: Option<String>,
    pub tool_calls: Option<String>,
    pub timestamp: f64,
}

fn export_hermes_sessions(
    db_path: &Path,
    root: &str,
    include_all: bool,
) -> Option<Vec<HermesExportedSession>> {
    let conn = open_hermes_ro(db_path)?;
    let (family_root, like) = hermes_family_params(root);
    let mut sql = String::from(
        "SELECT id, source, parent_session_id, started_at, ended_at, end_reason, \
                COALESCE(message_count, 0), title, cwd, git_branch, git_repo_root, archived \
         FROM sessions \
         WHERE source = 'cli' AND archived = 0 \
           AND (cwd = ?1 OR cwd LIKE ?2 ESCAPE '\\') \
         ORDER BY started_at DESC",
    );
    if !include_all {
        sql.push_str(" LIMIT 1");
    }
    let mut stmt = conn.prepare(&sql).ok()?;
    let rows = stmt
        .query_map(rusqlite::params![family_root, like], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, f64>(3)?,
                row.get::<_, Option<f64>>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, i64>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, Option<String>>(8)?,
                row.get::<_, Option<String>>(9)?,
                row.get::<_, Option<String>>(10)?,
                row.get::<_, i64>(11)?,
            ))
        })
        .ok()?;

    let mut out = Vec::new();
    for row in rows.flatten() {
        let (
            id,
            source,
            parent_session_id,
            started_at,
            ended_at,
            end_reason,
            message_count,
            title,
            cwd,
            git_branch,
            git_repo_root,
            archived,
        ) = row;
        let messages = load_hermes_messages(&conn, &id).unwrap_or_default();
        out.push(HermesExportedSession {
            id,
            source,
            parent_session_id,
            started_at,
            ended_at,
            end_reason,
            message_count,
            title,
            cwd,
            git_branch,
            git_repo_root,
            archived,
            messages,
        });
    }
    Some(out)
}

fn load_hermes_messages(
    conn: &rusqlite::Connection,
    session_id: &str,
) -> Result<Vec<HermesExportedMessage>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT role, content, tool_calls, timestamp FROM messages \
         WHERE session_id = ?1 ORDER BY timestamp ASC, id ASC",
    )?;
    let rows = stmt.query_map(rusqlite::params![session_id], |row| {
        Ok(HermesExportedMessage {
            role: row.get(0)?,
            content: row.get(1)?,
            tool_calls: row.get(2)?,
            timestamp: row.get(3)?,
        })
    })?;
    Ok(rows.flatten().collect())
}

fn open_hermes_ro(db_path: &Path) -> Option<rusqlite::Connection> {
    if !db_path.is_file() {
        return None;
    }
    let raw = db_path.to_string_lossy();
    let mut escaped = String::with_capacity(raw.len() + 12);
    for ch in raw.chars() {
        match ch {
            '%' => escaped.push_str("%25"),
            '?' => escaped.push_str("%3F"),
            '#' => escaped.push_str("%23"),
            _ => escaped.push(ch),
        }
    }
    let uri = format!("file:{escaped}?mode=ro");
    let conn = rusqlite::Connection::open_with_flags(
        uri,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
            | rusqlite::OpenFlags::SQLITE_OPEN_URI
            | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()?;
    let _ = conn.busy_timeout(std::time::Duration::from_millis(250));
    Some(conn)
}

fn hermes_family_params(project_path: &str) -> (String, String) {
    let root = resolve_root_project_path(project_path);
    let escaped = root
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    (root.to_string(), format!("{escaped}/%"))
}

// ── Shared peek helpers ───────────────────────────────────────────────

/// Read first JSONL line and pull a top-level string field.
fn peek_json_field(path: &Path, keys: &[&str]) -> Option<String> {
    let file = fs::File::open(path).ok()?;
    let mut reader = BufReader::new(file);
    let mut first_line = String::new();
    reader.read_line(&mut first_line).ok()?;
    let header: serde_json::Value = serde_json::from_str(first_line.trim()).ok()?;
    for key in keys {
        if let Some(s) = header.get(*key).and_then(|v| v.as_str()) {
            return Some(s.to_string());
        }
    }
    None
}

// ── Unpack helpers (used by unpack.rs) ────────────────────────────────

/// Percent-encode a path the way Grok does for session dir names
/// (`/Users/z/proj` → `%2FUsers%2Fz%2Fproj`). Unreserved RFC3986 chars
/// pass through; everything else becomes `%XX`.
pub(crate) fn grok_percent_encode_cwd(cwd: &str) -> String {
    let mut out = String::with_capacity(cwd.len() * 3);
    for byte in cwd.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => {
                out.push_str(&format!("%{byte:02X}"));
            }
        }
    }
    out
}

/// Map `providers/<rest>` archive path → on-disk path under `home`.
/// Re-keys Cursor MD5 dirs and Grok percent-encoded cwd segments for DEST.
///
/// `rest` is like `gemini/tmp/...`, `cursor/chats/<md5>/...`,
/// `grok/sessions/<encoded>/...`, `hermes/export.json`.
///
/// Returns `None` for Hermes export (handled as SQL merge, not a file
/// drop) and unrecognized/empty paths.
pub(super) fn reroot_provider(
    rest: &Path,
    home: &Path,
    _source_project_path: &str,
    dest_project_path: &str,
) -> Option<ProviderDest> {
    let mut comps = rest.components();
    let provider = comps.next()?.as_os_str().to_string_lossy().to_string();
    let after: PathBuf = comps.as_path().to_path_buf();
    if after.as_os_str().is_empty() && provider != "hermes" {
        return None;
    }

    match provider.as_str() {
        "hermes" => {
            // Always route hermes/export.json (or any hermes/*) through
            // the merge path — never drop a whole state.db.
            Some(ProviderDest::HermesExport)
        }
        "cursor" => {
            // cursor/chats/<src-md5>/<uuid>/store.db →
            // home/.cursor/chats/<dest-md5>/<uuid>/store.db
            reroot_cursor(&after, home, dest_project_path)
        }
        "grok" => {
            // grok/sessions/<src-encoded>/<uuid>/file →
            // home/.grok/sessions/<dest-encoded>/<uuid>/file
            reroot_grok(&after, home, dest_project_path)
        }
        "gemini" | "pi" | "codex" => {
            // home/.<provider>/<after...>
            Some(ProviderDest::File(
                home.join(format!(".{provider}")).join(&after),
            ))
        }
        _ => {
            // Unknown provider: best-effort home/.<provider>/<rest>
            Some(ProviderDest::File(
                home.join(format!(".{provider}")).join(&after),
            ))
        }
    }
}

/// Where a provider archive entry should land.
#[derive(Debug)]
pub(super) enum ProviderDest {
    /// Write bytes to this absolute path (after optional rewrite).
    File(PathBuf),
    /// Merge JSON export into dest `~/.hermes/state.db`.
    HermesExport,
}

fn reroot_cursor(after: &Path, home: &Path, dest_project_path: &str) -> Option<ProviderDest> {
    // after: chats/<src-md5>/<uuid>/store.db
    let mut comps = after.components();
    let chats = comps.next()?.as_os_str().to_string_lossy().to_string();
    if chats != "chats" {
        return Some(ProviderDest::File(
            home.join(".cursor").join(after),
        ));
    }
    let _src_md5 = comps.next()?; // discard source hash
    let remainder: PathBuf = comps.as_path().to_path_buf(); // <uuid>/store.db
    if remainder.as_os_str().is_empty() {
        return None;
    }
    let dest_root = resolve_root_project_path(dest_project_path);
    let dest_md5 = md5_hex(dest_root.as_bytes());
    Some(ProviderDest::File(
        home.join(".cursor")
            .join("chats")
            .join(dest_md5)
            .join(remainder),
    ))
}

fn reroot_grok(after: &Path, home: &Path, dest_project_path: &str) -> Option<ProviderDest> {
    // after: sessions/<src-encoded>/<uuid>/file
    let mut comps = after.components();
    let sessions = comps.next()?.as_os_str().to_string_lossy().to_string();
    if sessions != "sessions" {
        return Some(ProviderDest::File(home.join(".grok").join(after)));
    }
    let _src_encoded = comps.next()?; // discard source encoded cwd
    let remainder: PathBuf = comps.as_path().to_path_buf(); // <uuid>/file
    if remainder.as_os_str().is_empty() {
        return None;
    }
    let dest_encoded = grok_percent_encode_cwd(dest_project_path);
    Some(ProviderDest::File(
        home.join(".grok")
            .join("sessions")
            .join(dest_encoded)
            .join(remainder),
    ))
}

/// True when the archive path is under `providers/` and looks like a
/// text/jsonl session file that should get SOURCE→DEST path rewrite.
pub(super) fn provider_needs_path_rewrite(archive_path: &Path) -> bool {
    let s = archive_path.to_string_lossy();
    // rewrite jsonl / json (summary.json) / export handled separately
    s.ends_with(".jsonl")
        || s.ends_with("summary.json")
        || s.ends_with(".json") && !s.contains("auth")
}

/// Best-effort: apply SOURCE→DEST rewrite inside Cursor store.db bytes.
pub(super) fn provider_is_cursor_store_db(archive_path: &Path) -> bool {
    archive_path
        .to_string_lossy()
        .replace('\\', "/")
        .ends_with("store.db")
        && archive_path
            .components()
            .any(|c| c.as_os_str() == "cursor")
}

/// Merge a Gemini dest-path → slug entry into `~/.gemini/projects.json`
/// without clobbering unrelated projects. Uses the slug derived from the
/// first gemini file we just wrote under `tmp/<slug>/`.
pub(super) fn merge_gemini_projects_json(
    home: &Path,
    dest_project_path: &str,
    slug: &str,
) -> Result<(), String> {
    let path = home.join(".gemini").join("projects.json");
    let mut root: serde_json::Value = if path.is_file() {
        let content = fs::read_to_string(&path).map_err(|e| format!("read projects.json: {e}"))?;
        serde_json::from_str(&content).unwrap_or_else(|_| serde_json::json!({ "projects": {} }))
    } else {
        serde_json::json!({ "projects": {} })
    };
    if !root.get("projects").map(|p| p.is_object()).unwrap_or(false) {
        root["projects"] = serde_json::json!({});
    }
    if let Some(obj) = root.get_mut("projects").and_then(|p| p.as_object_mut()) {
        obj.insert(
            dest_project_path.to_string(),
            serde_json::Value::String(slug.to_string()),
        );
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("mkdir .gemini: {e}"))?;
    }
    let bytes = serde_json::to_vec_pretty(&root).map_err(|e| format!("serialize projects.json: {e}"))?;
    fs::write(&path, bytes).map_err(|e| format!("write projects.json: {e}"))?;
    Ok(())
}

/// Extract the Gemini slug from a provider rest path
/// `gemini/tmp/<slug>/chats/...`.
pub(super) fn gemini_slug_from_rest(rest: &Path) -> Option<String> {
    let mut comps = rest.components();
    let p = comps.next()?.as_os_str().to_string_lossy().to_string();
    if p != "gemini" {
        return None;
    }
    let tmp = comps.next()?.as_os_str().to_string_lossy().to_string();
    if tmp != "tmp" {
        return None;
    }
    comps
        .next()
        .map(|c| c.as_os_str().to_string_lossy().to_string())
}

/// Import Hermes export JSON into dest `state.db` (create minimal schema
/// if missing). INSERT only when session id is not already present.
/// Never replaces the whole DB. If locked, log via return Err.
pub(super) fn import_hermes_export(
    home: &Path,
    export_bytes: &[u8],
    source_project_path: &str,
    dest_project_path: &str,
) -> Result<usize, String> {
    let sessions: Vec<HermesExportedSession> = serde_json::from_slice(export_bytes)
        .map_err(|e| format!("parse hermes export: {e}"))?;
    if sessions.is_empty() {
        return Ok(0);
    }
    let db_path = home.join(".hermes").join("state.db");
    if let Some(parent) = db_path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("mkdir .hermes: {e}"))?;
    }
    let conn = rusqlite::Connection::open(&db_path)
        .map_err(|e| format!("open dest hermes state.db: {e}"))?;
    let _ = conn.busy_timeout(std::time::Duration::from_millis(500));
    // Ensure minimal schema (matches chat_history hermes fixtures).
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS sessions (
             id TEXT PRIMARY KEY,
             source TEXT NOT NULL,
             parent_session_id TEXT,
             started_at REAL NOT NULL,
             ended_at REAL,
             end_reason TEXT,
             message_count INTEGER DEFAULT 0,
             title TEXT,
             cwd TEXT,
             git_branch TEXT,
             git_repo_root TEXT,
             archived INTEGER NOT NULL DEFAULT 0
         );
         CREATE TABLE IF NOT EXISTS messages (
             id INTEGER PRIMARY KEY AUTOINCREMENT,
             session_id TEXT NOT NULL REFERENCES sessions(id),
             role TEXT NOT NULL,
             content TEXT,
             tool_calls TEXT,
             timestamp REAL NOT NULL
         );",
    )
    .map_err(|e| format!("ensure hermes schema: {e}"))?;

    let mut imported = 0usize;
    for mut s in sessions {
        // Rewrite cwd SOURCE→DEST.
        if let Some(ref cwd) = s.cwd {
            if !source_project_path.is_empty() && cwd.contains(source_project_path) {
                s.cwd = Some(cwd.replace(source_project_path, dest_project_path));
            }
        }
        if let Some(ref gr) = s.git_repo_root {
            if !source_project_path.is_empty() && gr.contains(source_project_path) {
                s.git_repo_root = Some(gr.replace(source_project_path, dest_project_path));
            }
        }
        // Skip if already present.
        let exists: bool = conn
            .query_row(
                "SELECT 1 FROM sessions WHERE id = ?1 LIMIT 1",
                rusqlite::params![s.id],
                |_| Ok(true),
            )
            .unwrap_or(false);
        if exists {
            continue;
        }
        conn.execute(
            "INSERT INTO sessions (id, source, parent_session_id, started_at, ended_at, \
             end_reason, message_count, title, cwd, git_branch, git_repo_root, archived) \
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
            rusqlite::params![
                s.id,
                s.source,
                s.parent_session_id,
                s.started_at,
                s.ended_at,
                s.end_reason,
                s.message_count,
                s.title,
                s.cwd,
                s.git_branch,
                s.git_repo_root,
                s.archived,
            ],
        )
        .map_err(|e| format!("insert hermes session {}: {e}", s.id))?;
        for m in &s.messages {
            let mut content = m.content.clone();
            if let Some(ref c) = content {
                if !source_project_path.is_empty() && c.contains(source_project_path) {
                    content = Some(c.replace(source_project_path, dest_project_path));
                }
            }
            conn.execute(
                "INSERT INTO messages (session_id, role, content, tool_calls, timestamp) \
                 VALUES (?1,?2,?3,?4,?5)",
                rusqlite::params![s.id, m.role, content, m.tool_calls, m.timestamp],
            )
            .map_err(|e| format!("insert hermes message for {}: {e}", s.id))?;
        }
        imported += 1;
    }
    Ok(imported)
}

#[cfg(test)]
mod unit_tests {
    use super::*;

    #[test]
    fn grok_percent_encode_matches_fixture_style() {
        assert_eq!(
            grok_percent_encode_cwd("/Users/z/proj-grok"),
            "%2FUsers%2Fz%2Fproj-grok"
        );
    }

    #[test]
    fn reroot_cursor_rekeys_md5() {
        let home = Path::new("/tmp/home");
        let src = "/src/proj";
        let dest = "/dest/proj";
        let src_md5 = md5_hex(src.as_bytes());
        let dest_md5 = md5_hex(dest.as_bytes());
        let rest = PathBuf::from(format!("cursor/chats/{src_md5}/uuid-1/store.db"));
        match reroot_provider(&rest, home, src, dest) {
            Some(ProviderDest::File(p)) => {
                let s = p.to_string_lossy().replace('\\', "/");
                assert!(
                    s.ends_with(&format!(".cursor/chats/{dest_md5}/uuid-1/store.db")),
                    "got {s}"
                );
                assert!(!s.contains(&src_md5) || src_md5 == dest_md5);
            }
            other => panic!("expected File, got {other:?}"),
        }
    }

    #[test]
    fn reroot_hermes_is_export() {
        let rest = Path::new("hermes/export.json");
        match reroot_provider(rest, Path::new("/h"), "/a", "/b") {
            Some(ProviderDest::HermesExport) => {}
            other => panic!("expected HermesExport, got {other:?}"),
        }
    }
}
