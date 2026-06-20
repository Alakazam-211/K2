//! Chat-history helpers shared between Tauri + daemon.
//!
//! When a heartbeat wake spawns `claude` via
//! `k2_daemon::wake_headless::spawn_wake_headless` (moved to the
//! daemon in 0.37.0), we need to save the provider's new session
//! ID on the `agent_sessions.session_id` row ~5 seconds later so the
//! *next* wake can `--resume <id>` into the same chat instead of
//! starting fresh. That requires scanning the provider's own history
//! file (Claude: `~/.claude/history.jsonl`; Cursor: `~/.cursor/chats/
//! <hash>/*/store.db`) to find the most recent session for this
//! project path.
//!
//! The scan is pure filesystem — zero Tauri dependencies — so it lives
//! here and gets called from both the daemon's wake path and the
//! Tauri-app UI's session-rediscovery code. The corresponding
//! `#[tauri::command]` wrapper in `src-tauri/src/commands/chat_history.rs`
//! is now a three-line forward.

use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// Strip a `/.worktrees/<branch>` suffix to get the root project path.
/// `/repo/.worktrees/feature-x` -> `/repo`. Used so every worktree of
/// the same project participates in session discovery.
pub fn resolve_root_project_path(path: &str) -> &str {
    if let Some(idx) = path.find("/.worktrees/") {
        &path[..idx]
    } else {
        path
    }
}

/// Does `session_project` belong to the `root` project family — the
/// root itself OR any worktree under it?
pub fn matches_project_family(session_project: &str, root: &str) -> bool {
    session_project == root
        || session_project.starts_with(&format!("{}/.worktrees/", root))
}

/// `~/.claude/history.jsonl` — where Claude Code appends one JSON
/// object per launch-prompt pair. `None` only if we can't find the
/// user's home directory.
pub fn claude_history_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".claude").join("history.jsonl"))
}

/// Convert a project path to Claude Code's project-slug directory name —
/// the `~/.claude/projects/<slug>/` folder Claude writes a workspace's
/// sessions into.
///
/// **The rule (empirically verified against `claude -p` on disk):** Claude
/// (a) canonicalizes the path (resolving symlinks, e.g. `/tmp`→`/private/tmp`,
/// `/var`→`/private/var`), then (b) maps EVERY character that isn't
/// `[a-zA-Z0-9]` or `-` to `-`. So `/`, `.`, `_`, ` ` all collapse to `-`,
/// and `--` runs (e.g. for a hidden `/.k2so` segment) are preserved as-is.
/// A live probe in `/tmp/k2so_slug_probe_a_b/has_under_score.dotted`
/// produced `-private-tmp-k2so-slug-probe-a-b-has-under-score-dotted`.
///
/// **Why this changed (GH#25 root cause):** the pre-0.39.44 encoder used
/// `replace("/.","/-").replace('/',"-").replace(' ',"-")`, which PRESERVED
/// `_` and any mid-component `.` (and never canonicalized symlinks). So for
/// any workspace path containing `_`, a dotted path component, or a
/// symlinked prefix, K2SO computed a DIFFERENT slug than Claude — the
/// bundler enumerated `~/.claude/projects/<K2SO-slug>/` (which didn't exist)
/// and got 0 sessions, unpack wrote to the wrong slug dir, and `/resume` +
/// the pinned-chat dropdown's exists-check never saw Claude's real history.
/// Every caller of this fn (clone inventory/unpack/repair, resume,
/// exists-check, newest-on-disk) uses it to FIND Claude's dirs, so matching
/// Claude's encoder is a pure bug-fix; plain paths (no `_`/dot/symlink) are
/// unchanged.
///
/// `canonicalize` only succeeds for paths that exist on disk; it FAILS on
/// non-existent paths (test fixtures, not-yet-created dests) → we fall back
/// to the literal input. The char-map then still reproduces Claude's output
/// for the no-symlink case (`/`→`-`, `.`→`-`, ` `→`-`, `_`→`-`).
pub fn claude_project_hash(project_path: &str) -> String {
    let canonical = std::fs::canonicalize(project_path)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| project_path.to_string());
    canonical
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// Convert a project path to Cursor's chat-directory hash.
/// `Users-z3thon-DevProjects-K2SO` — strip leading `/`, then slashes
/// to dashes.
pub fn cursor_project_hash(project_path: &str) -> String {
    project_path.trim_start_matches('/').replace('/', "-")
}

/// Return the most recent Claude session ID for a project path.
///
/// Reads the last 64 KiB of `~/.claude/history.jsonl` (cap against a
/// pathologically large file), filters by project family, picks the
/// highest-timestamp entry, then verifies the corresponding session
/// JSONL file actually exists on disk before returning.
///
/// The existence check matters: a session ID appears in history the
/// moment Claude launches, but the session `.jsonl` file is only
/// written once the user submits a prompt. If the user opened the
/// session but never typed, `--resume <id>` would fail with "No
/// conversation found".
pub fn detect_claude_session(project_path: &str) -> Option<String> {
    let path = claude_history_path()?;
    let file = File::open(&path).ok()?;

    let metadata = file.metadata().ok()?;
    let file_size = metadata.len();
    let read_from = if file_size > 65536 {
        file_size - 65536
    } else {
        0
    };

    let mut file = file;
    file.seek(SeekFrom::Start(read_from)).ok()?;

    let mut buf = String::new();
    file.read_to_string(&mut buf).ok()?;

    let is_subpath =
        project_path.contains("/.worktrees/")
            || project_path.contains("/.k2/")
            || project_path.contains("/.k2so/");
    let root = resolve_root_project_path(project_path);
    let mut best_session: Option<(i64, String)> = None;

    for line in buf.lines() {
        let parsed: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let project = parsed.get("project").and_then(|v| v.as_str()).unwrap_or("");
        if is_subpath {
            if project != project_path {
                continue;
            }
        } else if !matches_project_family(project, root) {
            continue;
        }

        let session_id = match parsed.get("sessionId").and_then(|v| v.as_str()) {
            Some(s) => s.to_string(),
            None => continue,
        };

        let timestamp = parsed.get("timestamp").and_then(|v| v.as_i64()).unwrap_or(0);

        match &best_session {
            Some((best_ts, _)) if timestamp > *best_ts => {
                best_session = Some((timestamp, session_id));
            }
            None => {
                best_session = Some((timestamp, session_id));
            }
            _ => {}
        }
    }

    best_session.and_then(|(_, id)| {
        let home = dirs::home_dir()?;
        let project_hash = claude_project_hash(resolve_root_project_path(project_path));
        let projects_dir = home.join(".claude").join("projects");

        // Fast path: most workspaces store their session under exactly
        // `<projects>/<hash>/<id>.jsonl`. Hit that first with a single
        // stat. Avoids walking the entire `.claude/projects/` directory
        // on every call — that walk costs O(N) stats per project the
        // user has ever opened, and `AgentChatPane`'s 5s detection
        // poll multiplies it across every chat tab. Worktree-suffixed
        // dirs (`<hash>-<branch>`) still need the read_dir fallback.
        let direct = projects_dir
            .join(&project_hash)
            .join(format!("{}.jsonl", id));
        if direct.exists() {
            return Some(id);
        }

        // Fallback: scan for `<hash>-<branch>` worktree variants. Same
        // O(N) cost as before, but only paid when the direct path
        // misses — which is the worktree case, not the common case.
        if let Ok(entries) = fs::read_dir(&projects_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with(&format!("{}-", project_hash)) {
                    let session_file = entry.path().join(format!("{}.jsonl", id));
                    if session_file.exists() {
                        return Some(id);
                    }
                }
            }
        }
        None
    })
}

/// Return the Claude session ID for a project path whose history
/// entry timestamp is **closest to `target_ms`**.
///
/// Disambiguates concurrent spawns: when two heartbeats fire on the
/// same agent within a short window, each spawn's deferred-save
/// thread calls this with its own spawn timestamp, picking the
/// session id whose creation is nearest in time to its own spawn.
/// Without this, both threads would race-pick the highest-timestamp
/// session via [`detect_claude_session`] and stamp the same id on
/// both heartbeat rows.
///
/// `target_ms` is unix-epoch milliseconds (e.g.
/// `chrono::Utc::now().timestamp_millis()` captured at spawn).
/// Considers only sessions whose history timestamp is in
/// `[target_ms - 60_000, target_ms + 60_000]` so we don't pick up
/// an unrelated old session if Claude failed to write history at
/// all for the new spawn.
pub fn detect_claude_session_near(
    project_path: &str,
    target_ms: i64,
) -> Option<String> {
    let path = claude_history_path()?;
    let file = File::open(&path).ok()?;

    let metadata = file.metadata().ok()?;
    let file_size = metadata.len();
    let read_from = if file_size > 65536 {
        file_size - 65536
    } else {
        0
    };

    let mut file = file;
    file.seek(SeekFrom::Start(read_from)).ok()?;

    let mut buf = String::new();
    file.read_to_string(&mut buf).ok()?;

    let is_subpath =
        project_path.contains("/.worktrees/")
            || project_path.contains("/.k2/")
            || project_path.contains("/.k2so/");
    let root = resolve_root_project_path(project_path);
    const WINDOW_MS: i64 = 60_000;
    let mut best: Option<(i64, String)> = None; // (|distance|, id)

    for line in buf.lines() {
        let parsed: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let project = parsed.get("project").and_then(|v| v.as_str()).unwrap_or("");
        if is_subpath {
            if project != project_path {
                continue;
            }
        } else if !matches_project_family(project, root) {
            continue;
        }

        let session_id = match parsed.get("sessionId").and_then(|v| v.as_str()) {
            Some(s) => s.to_string(),
            None => continue,
        };

        let ts = parsed.get("timestamp").and_then(|v| v.as_i64()).unwrap_or(0);
        // Claude history timestamps in this codebase are observed in
        // milliseconds. Defensive: if the value looks like seconds,
        // upscale.
        let ts_ms = if ts < 10_000_000_000 { ts * 1000 } else { ts };

        let distance = (ts_ms - target_ms).abs();
        if distance > WINDOW_MS {
            continue;
        }

        match &best {
            Some((bd, _)) if distance < *bd => {
                best = Some((distance, session_id));
            }
            None => {
                best = Some((distance, session_id));
            }
            _ => {}
        }
    }

    best.and_then(|(_, id)| {
        let home = dirs::home_dir()?;
        let project_hash = claude_project_hash(resolve_root_project_path(project_path));
        let projects_dir = home.join(".claude").join("projects");
        if let Ok(entries) = fs::read_dir(&projects_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name == project_hash
                    || name.starts_with(&format!("{}-", project_hash))
                {
                    if entry.path().join(format!("{}.jsonl", id)).exists() {
                        return Some(id);
                    }
                }
            }
        }
        None
    })
}

/// Does a Claude session `.jsonl` file exist on disk for this
/// `session_id` + `project_path` (including any worktree siblings)?
/// Used before a `--resume` to avoid "No conversation found" when
/// the DB holds a stale session_id (workspace remove+readd,
/// Claude-side pruning, migrations, etc.).
pub fn claude_session_file_exists(session_id: &str, project_path: &str) -> bool {
    let Some(home) = dirs::home_dir() else {
        return false;
    };
    let project_hash = claude_project_hash(resolve_root_project_path(project_path));
    let projects_dir = home.join(".claude").join("projects");
    let Ok(entries) = fs::read_dir(&projects_dir) else {
        return false;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name == project_hash || name.starts_with(&format!("{}-", project_hash)) {
            if entry
                .path()
                .join(format!("{}.jsonl", session_id))
                .exists()
            {
                return true;
            }
        }
    }
    false
}

/// The most-recently-modified Claude session `.jsonl` on disk for this
/// `project_path` (the bare `<hash>` dir + any `<hash>-<branch>`
/// worktree siblings) — i.e. the session a running `claude` is
/// actively appending to, or the latest prior conversation. Returns the
/// session id (file stem), or `None` when the workspace has no on-disk
/// session at all (a genuinely brand-new workspace).
///
/// GH#24: the pinned-chat resolver
/// ([`crate::workspace::resume_chat::resolve_resume_chat_args`]) uses
/// this to **resume** the real session instead of minting a throwaway
/// `--session-id` when the saved id's JSONL is missing. Minting +
/// overwriting an unconfirmed id that a *reused* (already-live) PTY
/// never actually runs left its JSONL absent forever, so every
/// subsequent resolve re-minted — an endless loop on remote/companion
/// clients, which re-ask on each reconnect. The dir set here mirrors
/// [`claude_session_file_exists`] exactly, so any id returned is
/// guaranteed to pass that existence check on the next resolve (the
/// happy path), making the resolver convergent.
///
/// Zero-byte files are skipped: a `--session-id <new>` that was
/// pre-allocated but never run leaves an empty file that `--resume`
/// would reject with "No conversation found".
pub fn newest_claude_session_on_disk(project_path: &str) -> Option<String> {
    let home = dirs::home_dir()?;
    let project_hash = claude_project_hash(resolve_root_project_path(project_path));
    let projects_dir = home.join(".claude").join("projects");
    newest_session_in_projects_dir(&projects_dir, &project_hash)
}

/// Inner, dependency-free core of [`newest_claude_session_on_disk`] —
/// takes the `~/.claude/projects` dir explicitly so it's testable
/// without mutating the process `$HOME`.
fn newest_session_in_projects_dir(projects_dir: &Path, project_hash: &str) -> Option<String> {
    let entries = fs::read_dir(projects_dir).ok()?;
    let mut best: Option<(std::time::SystemTime, String)> = None;

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !(name == *project_hash || name.starts_with(&format!("{}-", project_hash))) {
            continue;
        }
        let Ok(session_files) = fs::read_dir(entry.path()) else {
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
            if meta.len() == 0 {
                continue;
            }
            let Ok(mtime) = meta.modified() else {
                continue;
            };
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            match &best {
                Some((best_mtime, _)) if mtime <= *best_mtime => {}
                _ => best = Some((mtime, stem.to_string())),
            }
        }
    }

    best.map(|(_, id)| id)
}

/// Return the most recent Cursor chat ID for a project path (by
/// store.db modification time across all matching hash dirs,
/// including worktree variants). None if Cursor has no data for this
/// project.
pub fn detect_cursor_session(project_path: &str) -> Option<String> {
    let cursor_chats_dir = dirs::home_dir()?.join(".cursor").join("chats");
    let root = resolve_root_project_path(project_path);
    let root_hash = cursor_project_hash(root);

    let hash_dirs: Vec<PathBuf> = match fs::read_dir(&cursor_chats_dir) {
        Ok(entries) => entries
            .filter_map(|e| e.ok())
            .filter(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                e.path().is_dir()
                    && (name == root_hash
                        || name.starts_with(&format!("{}-.worktrees-", root_hash)))
            })
            .map(|e| e.path())
            .collect(),
        Err(_) => return None,
    };

    let mut best: Option<(std::time::SystemTime, String)> = None;

    for hash_dir in hash_dirs {
        let entries = match fs::read_dir(&hash_dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            if !entry.path().is_dir() {
                continue;
            }
            let store_db = entry.path().join("store.db");
            if let Ok(meta) = fs::metadata(&store_db) {
                if let Ok(modified) = meta.modified() {
                    let chat_id = entry.file_name().to_string_lossy().to_string();
                    match &best {
                        Some((best_time, _)) if modified > *best_time => {
                            best = Some((modified, chat_id));
                        }
                        None => {
                            best = Some((modified, chat_id));
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    best.map(|(_, id)| id)
}

/// Return the most recent Gemini session uuid for a project path.
///
/// Gemini's storage layout (verified against a live install):
///   ~/.gemini/projects.json     — { "projects": { "/abs/cwd": "<slug>" } }
///   ~/.gemini/tmp/<slug>/chats/session-<iso>-<short-uuid>.jsonl
///
/// The on-disk filename only carries an 8-char prefix of the uuid, so we
/// MUST read line 1 of the JSONL header to extract the full `sessionId`
/// — that's what `gemini --resume <uuid>` expects. The "most recent"
/// session is picked by file mtime across every project-family slug
/// (matching root + worktree paths the same way detect_cursor_session
/// does).
pub fn detect_gemini_session(project_path: &str) -> Option<String> {
    let home = dirs::home_dir()?;
    let projects_json = home.join(".gemini").join("projects.json");
    let tmp_dir = home.join(".gemini").join("tmp");
    if !tmp_dir.exists() {
        return None;
    }

    let content = fs::read_to_string(&projects_json).ok()?;
    let parsed: serde_json::Value = serde_json::from_str(&content).ok()?;
    let projects_obj = parsed.get("projects")?.as_object()?;

    let root = resolve_root_project_path(project_path);
    let mut best: Option<(std::time::SystemTime, std::path::PathBuf)> = None;

    // Scan every slug whose cwd is in this project's family (root +
    // worktrees), then pick the newest session file.
    for (cwd, slug_v) in projects_obj {
        if !matches_project_family(cwd, root) {
            continue;
        }
        let slug = match slug_v.as_str() {
            Some(s) => s,
            None => continue,
        };
        let chats_dir = tmp_dir.join(slug).join("chats");
        let entries = match fs::read_dir(&chats_dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
                continue;
            }
            if let Ok(meta) = fs::metadata(&path) {
                if let Ok(modified) = meta.modified() {
                    match &best {
                        Some((best_time, _)) if modified > *best_time => {
                            best = Some((modified, path));
                        }
                        None => {
                            best = Some((modified, path));
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    // Read line 1 → extract the full uuid from `sessionId`.
    let path = best.map(|(_, p)| p)?;
    let file = std::fs::File::open(&path).ok()?;
    let mut reader = std::io::BufReader::new(file);
    let mut first_line = String::new();
    use std::io::BufRead;
    reader.read_line(&mut first_line).ok()?;
    let header: serde_json::Value = serde_json::from_str(first_line.trim()).ok()?;
    header
        .get("sessionId")
        .and_then(|v| v.as_str())
        .map(String::from)
}

/// Return the most recent Pi session uuid for a project path.
///
/// Pi's storage layout (verified against pi-mono — github.com/badlogic/pi-mono):
///   ~/.pi/agent/sessions/<cwd-slug>/<ISO-ts>_<uuidv7>.jsonl
///   line 1: {"type":"session","id":"<uuid>","cwd":"/abs/path","timestamp":…}
///
/// Pi's slug encoding is reversible (`/`→`-` with `--…--` wrapping) but
/// we don't depend on it: walk every slug dir, read line 1's literal
/// `cwd`, and keep the matching ones. More robust across worktrees and
/// any encoding edge case. Picks the newest by file mtime, then reads
/// line 1 again to extract the full uuid for `pi --session <uuid>`.
pub fn detect_pi_session(project_path: &str) -> Option<String> {
    let home = dirs::home_dir()?;
    let sessions_root = home.join(".pi").join("agent").join("sessions");
    if !sessions_root.exists() {
        return None;
    }

    let root = resolve_root_project_path(project_path);
    let mut best: Option<(std::time::SystemTime, std::path::PathBuf)> = None;

    let slug_dirs = fs::read_dir(&sessions_root).ok()?;
    for slug_entry in slug_dirs.flatten() {
        let slug_path = slug_entry.path();
        if !slug_path.is_dir() {
            continue;
        }
        let session_files = match fs::read_dir(&slug_path) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for f_entry in session_files.flatten() {
            let path = f_entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
                continue;
            }

            // Peek at line 1 to confirm cwd is in our project family
            // before considering this file as a candidate.
            let file = match std::fs::File::open(&path) {
                Ok(f) => f,
                Err(_) => continue,
            };
            let mut reader = std::io::BufReader::new(file);
            let mut first_line = String::new();
            use std::io::BufRead;
            if reader.read_line(&mut first_line).is_err() {
                continue;
            }
            let header: serde_json::Value = match serde_json::from_str(first_line.trim()) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let cwd = match header.get("cwd").and_then(|v| v.as_str()) {
                Some(s) => s,
                None => continue,
            };
            if !matches_project_family(cwd, root) {
                continue;
            }

            let modified = match fs::metadata(&path).and_then(|m| m.modified()) {
                Ok(t) => t,
                Err(_) => continue,
            };
            match &best {
                Some((best_time, _)) if modified > *best_time => {
                    best = Some((modified, path));
                }
                None => {
                    best = Some((modified, path));
                }
                _ => {}
            }
        }
    }

    // Re-read the winner's line 1 to extract the uuid.
    let path = best.map(|(_, p)| p)?;
    let file = std::fs::File::open(&path).ok()?;
    let mut reader = std::io::BufReader::new(file);
    let mut first_line = String::new();
    use std::io::BufRead;
    reader.read_line(&mut first_line).ok()?;
    let header: serde_json::Value = serde_json::from_str(first_line.trim()).ok()?;
    header.get("id").and_then(|v| v.as_str()).map(String::from)
}

/// Return the most recent Codex session uuid for a project path.
///
/// Codex layout (0.125+):
///   ~/.codex/sessions/YYYY/MM/DD/rollout-<ISO>-<uuidv7>.jsonl
///   line 1: {"type":"session_meta","payload":{"id","timestamp","cwd",…}}
///
/// Walks the dated partitions, reads each rollout's line 1 to filter by
/// cwd (project family), picks the file with newest mtime, returns its
/// `payload.id` for `codex resume <uuid>`.
pub fn detect_codex_session(project_path: &str) -> Option<String> {
    let home = dirs::home_dir()?;
    let sessions_root = home.join(".codex").join("sessions");
    if !sessions_root.exists() {
        return None;
    }

    let root = resolve_root_project_path(project_path);
    let mut best: Option<(std::time::SystemTime, std::path::PathBuf)> = None;

    let years = fs::read_dir(&sessions_root).ok()?;
    for year_entry in years.flatten() {
        if !year_entry.path().is_dir() {
            continue;
        }
        let months = match fs::read_dir(year_entry.path()) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for month_entry in months.flatten() {
            if !month_entry.path().is_dir() {
                continue;
            }
            let days = match fs::read_dir(month_entry.path()) {
                Ok(e) => e,
                Err(_) => continue,
            };
            for day_entry in days.flatten() {
                if !day_entry.path().is_dir() {
                    continue;
                }
                let files = match fs::read_dir(day_entry.path()) {
                    Ok(e) => e,
                    Err(_) => continue,
                };
                for f_entry in files.flatten() {
                    let path = f_entry.path();
                    if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
                        continue;
                    }
                    let file = match std::fs::File::open(&path) {
                        Ok(f) => f,
                        Err(_) => continue,
                    };
                    let mut reader = std::io::BufReader::new(file);
                    let mut first_line = String::new();
                    use std::io::BufRead;
                    if reader.read_line(&mut first_line).is_err() {
                        continue;
                    }
                    let header: serde_json::Value = match serde_json::from_str(first_line.trim()) {
                        Ok(v) => v,
                        Err(_) => continue,
                    };
                    let cwd = match header
                        .get("payload")
                        .and_then(|p| p.get("cwd"))
                        .and_then(|v| v.as_str())
                    {
                        Some(s) => s,
                        None => continue,
                    };
                    if !matches_project_family(cwd, root) {
                        continue;
                    }
                    let modified = match fs::metadata(&path).and_then(|m| m.modified()) {
                        Ok(t) => t,
                        Err(_) => continue,
                    };
                    match &best {
                        Some((best_time, _)) if modified > *best_time => {
                            best = Some((modified, path));
                        }
                        None => {
                            best = Some((modified, path));
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    let path = best.map(|(_, p)| p)?;
    let file = std::fs::File::open(&path).ok()?;
    let mut reader = std::io::BufReader::new(file);
    let mut first_line = String::new();
    use std::io::BufRead;
    reader.read_line(&mut first_line).ok()?;
    let header: serde_json::Value = serde_json::from_str(first_line.trim()).ok()?;
    header
        .get("payload")
        .and_then(|p| p.get("id"))
        .and_then(|v| v.as_str())
        .map(String::from)
}

/// Provider dispatcher used by the daemon's post-spawn session-save
/// task (and the Tauri UI's session-rediscovery). Returns `Ok(None)`
/// when no session is detected — distinct from `Err` so callers can
/// distinguish "nothing to save" from "detection broke."
pub fn detect_active_session(
    provider: &str,
    project_path: &str,
) -> Result<Option<String>, String> {
    let session = match provider {
        "claude" => detect_claude_session(project_path),
        "cursor" => detect_cursor_session(project_path),
        "gemini" => detect_gemini_session(project_path),
        "pi" => detect_pi_session(project_path),
        "codex" => detect_codex_session(project_path),
        _ => None,
    };
    Ok(session)
}

// ─────────────────────────────────────────────────────────────────────
// Phase 2 Unit 6 — full IDE-history parsing surface
// ─────────────────────────────────────────────────────────────────────
//
// The functions below are the daemon-side migration of the ~1700 LoC
// of parse-and-aggregate code that lived in
// `src-tauri/src/commands/chat_history.rs`. They preserve the exact
// response shapes the renderer's `ChatHistory.tsx` component already
// consumes — see `ChatSession`, `ChatStoragePaths`, `CursorIdeSession`.

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatSession {
    pub session_id: String,
    pub project: String,
    pub title: String,
    pub timestamp: i64,
    pub provider: String,
    pub message_count: usize,
    /// Worktree branch name if this session was created in a worktree.
    pub origin_branch: Option<String>,
}

struct SessionAccumulator {
    session_id: String,
    project: String,
    first_display: String,
    first_timestamp: i64,
    last_timestamp: i64,
    count: usize,
}

fn extract_worktree_branch(project: &str) -> Option<String> {
    project
        .find("/.worktrees/")
        .map(|idx| project[idx + 12..].to_string())
}

// ── Claude history parsing ──────────────────────────────────────────────

pub fn parse_claude_sessions(project_filter: Option<&str>) -> Result<Vec<ChatSession>, String> {
    let path = match claude_history_path() {
        Some(p) => p,
        None => return Ok(vec![]),
    };
    let file = match File::open(&path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(format!("Failed to open history file: {}", e)),
    };
    let reader = BufReader::new(file);
    let mut sessions: HashMap<String, SessionAccumulator> = HashMap::new();
    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => continue,
        };
        if line.trim().is_empty() {
            continue;
        }
        let parsed: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let session_id = match parsed.get("sessionId").and_then(|v| v.as_str()) {
            Some(s) => s.to_string(),
            None => continue,
        };
        let project = parsed
            .get("project")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if let Some(filter) = project_filter {
            let root = resolve_root_project_path(filter);
            if !matches_project_family(&project, root) {
                continue;
            }
        }
        let display = parsed
            .get("display")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let timestamp = parsed.get("timestamp").and_then(|v| v.as_i64()).unwrap_or(0);
        sessions
            .entry(session_id.clone())
            .and_modify(|acc| {
                acc.count += 1;
                if timestamp > acc.last_timestamp {
                    acc.last_timestamp = timestamp;
                }
                if timestamp < acc.first_timestamp {
                    acc.first_timestamp = timestamp;
                    acc.first_display = display.clone();
                }
            })
            .or_insert(SessionAccumulator {
                session_id,
                project,
                first_display: display,
                first_timestamp: timestamp,
                last_timestamp: timestamp,
                count: 1,
            });
    }
    Ok(sessions
        .into_values()
        .map(|acc| {
            let title = if acc.first_display.len() > 80 {
                let truncated: String = acc.first_display.chars().take(77).collect();
                format!("{}...", truncated)
            } else {
                acc.first_display
            };
            ChatSession {
                origin_branch: extract_worktree_branch(&acc.project),
                session_id: acc.session_id,
                project: acc.project,
                title,
                timestamp: acc.last_timestamp,
                provider: "claude".to_string(),
                message_count: acc.count,
            }
        })
        .collect())
}

// ── Cursor chat parsing ─────────────────────────────────────────────────

fn read_cursor_chat_meta(store_db: &std::path::Path) -> Option<(String, i64)> {
    let conn = rusqlite::Connection::open_with_flags(
        store_db,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()?;
    let hex_value: String = conn
        .query_row("SELECT value FROM meta WHERE key = '0'", [], |row| row.get(0))
        .ok()?;
    let chars: Vec<char> = hex_value.chars().collect();
    if chars.len() % 2 != 0 {
        return None;
    }
    let mut bytes = Vec::with_capacity(chars.len() / 2);
    for chunk in chars.chunks(2) {
        let s: String = chunk.iter().collect();
        bytes.push(u8::from_str_radix(&s, 16).ok()?);
    }
    let json_str = String::from_utf8(bytes).ok()?;
    let parsed: serde_json::Value = serde_json::from_str(&json_str).ok()?;
    let name = parsed
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("Untitled")
        .to_string();
    let timestamp = parsed
        .get("lastUpdatedAt")
        .and_then(|v| v.as_i64())
        .or_else(|| parsed.get("createdAt").and_then(|v| v.as_i64()))
        .unwrap_or(0);
    Some((name, timestamp))
}

pub fn parse_cursor_sessions(project_filter: Option<&str>) -> Result<Vec<ChatSession>, String> {
    let cursor_chats_dir = match dirs::home_dir() {
        Some(h) => h.join(".cursor").join("chats"),
        None => return Ok(vec![]),
    };
    if !cursor_chats_dir.exists() {
        return Ok(vec![]);
    }
    let mut best_by_id: HashMap<String, ChatSession> = HashMap::new();
    let hash_dirs: Vec<PathBuf> = if let Some(filter) = project_filter {
        let root = resolve_root_project_path(filter);
        let root_hash = md5_hex(root.as_bytes());
        let target_dir = cursor_chats_dir.join(&root_hash);
        if target_dir.is_dir() {
            vec![target_dir]
        } else {
            vec![]
        }
    } else {
        match fs::read_dir(&cursor_chats_dir) {
            Ok(entries) => entries
                .filter_map(|e| e.ok())
                .filter(|e| e.path().is_dir())
                .map(|e| e.path())
                .collect(),
            Err(_) => vec![],
        }
    };
    for hash_dir in hash_dirs {
        let chat_dirs = match fs::read_dir(&hash_dir) {
            Ok(entries) => entries
                .filter_map(|e| e.ok())
                .filter(|e| e.path().is_dir())
                .collect::<Vec<_>>(),
            Err(_) => continue,
        };
        for chat_entry in chat_dirs {
            let chat_path = chat_entry.path();
            let chat_id = match chat_path.file_name() {
                Some(n) => n.to_string_lossy().to_string(),
                None => continue,
            };
            let store_db = chat_path.join("store.db");
            if !store_db.exists() {
                continue;
            }
            let (title, timestamp) = match read_cursor_chat_meta(&store_db) {
                Some((name, meta_ts)) => {
                    let ts = if meta_ts > 0 {
                        meta_ts
                    } else {
                        fs::metadata(&store_db)
                            .ok()
                            .and_then(|m| m.modified().ok())
                            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                            .map(|d| d.as_millis() as i64)
                            .unwrap_or(0)
                    };
                    (name, ts)
                }
                None => {
                    let file_ts = fs::metadata(&store_db)
                        .ok()
                        .and_then(|m| m.modified().ok())
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_millis() as i64)
                        .unwrap_or(0);
                    let short_id = if chat_id.len() > 8 { &chat_id[..8] } else { &chat_id };
                    (format!("Cursor session {}", short_id), file_ts)
                }
            };
            let session = ChatSession {
                session_id: chat_id.clone(),
                project: String::new(),
                title,
                timestamp,
                provider: "cursor".to_string(),
                message_count: 0,
                origin_branch: None,
            };
            match best_by_id.get(&chat_id) {
                Some(existing) => {
                    let existing_is_generic = existing.title == "New Agent"
                        || existing.title.starts_with("Cursor session ")
                        || existing.title == "Untitled";
                    let new_is_named = session.title != "New Agent"
                        && !session.title.starts_with("Cursor session ")
                        && session.title != "Untitled";
                    if (new_is_named && existing_is_generic)
                        || (new_is_named == !existing_is_generic
                            && session.timestamp > existing.timestamp)
                    {
                        best_by_id.insert(chat_id, session);
                    }
                }
                None => {
                    best_by_id.insert(chat_id, session);
                }
            }
        }
    }
    Ok(best_by_id.into_values().collect())
}

// ── Cursor IDE workspace storage parsing ────────────────────────────────

pub fn parse_cursor_ide_sessions(project_filter: Option<&str>) -> Result<Vec<ChatSession>, String> {
    let workspace_dir = match dirs::home_dir() {
        Some(h) => h.join("Library/Application Support/Cursor/User/workspaceStorage"),
        None => return Ok(vec![]),
    };
    if !workspace_dir.exists() {
        return Ok(vec![]);
    }
    let mut results = Vec::new();
    let entries = match fs::read_dir(&workspace_dir) {
        Ok(e) => e,
        Err(_) => return Ok(vec![]),
    };
    for entry in entries.flatten() {
        let ws_path = entry.path();
        if !ws_path.is_dir() {
            continue;
        }
        let ws_json_path = ws_path.join("workspace.json");
        let ws_json = match fs::read_to_string(&ws_json_path) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let ws_data: serde_json::Value = match serde_json::from_str(&ws_json) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let folder_uri = match ws_data.get("folder").and_then(|v| v.as_str()) {
            Some(f) => f.to_string(),
            None => continue,
        };
        let folder_path = percent_decode_uri(&folder_uri);
        if let Some(filter) = project_filter {
            let root = resolve_root_project_path(filter);
            if !matches_project_family(&folder_path, root) {
                continue;
            }
        }
        let state_db_path = ws_path.join("state.vscdb");
        if !state_db_path.exists() {
            continue;
        }
        let conn = match rusqlite::Connection::open_with_flags(
            &state_db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        ) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let composer_json: String = match conn.query_row(
            "SELECT value FROM ItemTable WHERE key = 'composer.composerData'",
            [],
            |row| row.get(0),
        ) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let composer_data: serde_json::Value = match serde_json::from_str(&composer_json) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let composers = match composer_data.get("allComposers").and_then(|v| v.as_array()) {
            Some(arr) => arr,
            None => continue,
        };
        let project_display = folder_path
            .rsplit('/')
            .next()
            .unwrap_or(&folder_path)
            .to_string();
        for composer in composers {
            let composer_id = match composer.get("composerId").and_then(|v| v.as_str()) {
                Some(id) => id.to_string(),
                None => continue,
            };
            let name = composer
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("Untitled")
                .to_string();
            let title = if name.len() > 80 {
                let truncated: String = name.chars().take(77).collect();
                format!("{}...", truncated)
            } else {
                name
            };
            let timestamp = composer
                .get("lastUpdatedAt")
                .and_then(|v| v.as_i64())
                .or_else(|| composer.get("createdAt").and_then(|v| v.as_i64()))
                .unwrap_or(0);
            results.push(ChatSession {
                session_id: composer_id,
                project: project_display.clone(),
                origin_branch: extract_worktree_branch(&project_display),
                title,
                timestamp,
                provider: "cursor".to_string(),
                message_count: 0,
            });
        }
    }
    // Dedupe by composer_id: a single Cursor IDE composer can appear in
    // multiple workspaceStorage directories (e.g. when a user opens the
    // same project from two folder URIs, or when Cursor migrates state
    // between workspace IDs). Without dedup the history list shows the
    // same logical chat twice with identical session_ids → duplicate
    // React keys downstream. Mirrors the dedup in `parse_cursor_sessions`
    // and #550's fix in `parse_gemini_sessions`. Keep the entry with
    // the most recent `lastUpdatedAt` since that's the workspace the
    // user touched last.
    use std::collections::HashMap;
    let mut by_id: HashMap<String, ChatSession> = HashMap::new();
    for s in results {
        match by_id.get(&s.session_id) {
            Some(existing) if existing.timestamp >= s.timestamp => {}
            _ => {
                by_id.insert(s.session_id.clone(), s);
            }
        }
    }
    Ok(by_id.into_values().collect())
}

// ── Gemini chat parsing ─────────────────────────────────────────────────

fn parse_rfc3339_to_ms(s: &str) -> Option<i64> {
    if s.is_empty() {
        return None;
    }
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.timestamp_millis())
}

fn gemini_slug_to_cwd_map() -> HashMap<String, String> {
    let mut map = HashMap::new();
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return map,
    };
    let projects_json = home.join(".gemini").join("projects.json");
    let content = match fs::read_to_string(&projects_json) {
        Ok(c) => c,
        Err(_) => return map,
    };
    let parsed: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return map,
    };
    if let Some(obj) = parsed.get("projects").and_then(|v| v.as_object()) {
        for (cwd, slug_v) in obj {
            if let Some(slug) = slug_v.as_str() {
                map.insert(slug.to_string(), cwd.clone());
            }
        }
    }
    map
}

pub fn parse_gemini_sessions(project_filter: Option<&str>) -> Result<Vec<ChatSession>, String> {
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return Ok(vec![]),
    };
    let tmp_dir = home.join(".gemini").join("tmp");
    if !tmp_dir.exists() {
        return Ok(vec![]);
    }
    let slug_to_cwd = gemini_slug_to_cwd_map();
    let target_slugs: Vec<(String, String)> = if let Some(filter) = project_filter {
        let root = resolve_root_project_path(filter);
        slug_to_cwd
            .iter()
            .filter(|(_slug, cwd)| matches_project_family(cwd, root))
            .map(|(slug, cwd)| (slug.clone(), cwd.clone()))
            .collect()
    } else {
        slug_to_cwd
            .iter()
            .map(|(slug, cwd)| (slug.clone(), cwd.clone()))
            .collect()
    };
    let mut results = Vec::new();
    for (slug, cwd) in target_slugs {
        let chats_dir = tmp_dir.join(&slug).join("chats");
        if !chats_dir.is_dir() {
            continue;
        }
        let entries = match fs::read_dir(&chats_dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
                continue;
            }
            let file = match File::open(&path) {
                Ok(f) => f,
                Err(_) => continue,
            };
            let reader = BufReader::new(file);
            let mut lines = reader.lines();
            let header_line = match lines.next() {
                Some(Ok(l)) => l,
                _ => continue,
            };
            let header: serde_json::Value = match serde_json::from_str(&header_line) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let session_id = match header.get("sessionId").and_then(|v| v.as_str()) {
                Some(s) => s.to_string(),
                None => continue,
            };
            let mut latest_ts_ms = parse_rfc3339_to_ms(
                header
                    .get("lastUpdated")
                    .and_then(|v| v.as_str())
                    .unwrap_or(""),
            )
            .unwrap_or(0);
            let start_ts_ms = parse_rfc3339_to_ms(
                header
                    .get("startTime")
                    .and_then(|v| v.as_str())
                    .unwrap_or(""),
            )
            .unwrap_or(0);
            let mut title = String::new();
            let mut message_count: usize = 0;
            for line in lines.flatten() {
                let parsed: serde_json::Value = match serde_json::from_str(&line) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                if let Some(set) = parsed.get("$set") {
                    if let Some(s) = set.get("lastUpdated").and_then(|v| v.as_str()) {
                        if let Some(ms) = parse_rfc3339_to_ms(s) {
                            if ms > latest_ts_ms {
                                latest_ts_ms = ms;
                            }
                        }
                    }
                    continue;
                }
                let msg_type = parsed.get("type").and_then(|v| v.as_str()).unwrap_or("");
                if msg_type != "user" && msg_type != "gemini" {
                    continue;
                }
                message_count += 1;
                if title.is_empty() && msg_type == "user" {
                    let content = parsed.get("content");
                    let extracted = if let Some(arr) = content.and_then(|v| v.as_array()) {
                        arr.iter()
                            .find_map(|item| item.get("text").and_then(|v| v.as_str()))
                            .map(String::from)
                    } else {
                        content.and_then(|v| v.as_str()).map(String::from)
                    };
                    if let Some(s) = extracted {
                        title = s.trim().to_string();
                    }
                }
            }
            let timestamp = if latest_ts_ms > 0 {
                latest_ts_ms
            } else if start_ts_ms > 0 {
                start_ts_ms
            } else {
                fs::metadata(&path)
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_millis() as i64)
                    .unwrap_or(0)
            };
            let truncated_title = if title.is_empty() {
                "Untitled".to_string()
            } else if title.chars().count() > 80 {
                let truncated: String = title.chars().take(77).collect();
                format!("{}...", truncated)
            } else {
                title
            };
            results.push(ChatSession {
                session_id,
                project: cwd.clone(),
                origin_branch: extract_worktree_branch(&cwd),
                title: truncated_title,
                timestamp,
                provider: "gemini".to_string(),
                message_count,
            });
        }
    }
    // Dedupe by session_id: Gemini's CLI checkpoints a session into
    // multiple .jsonl files when the user resumes — every file in the
    // chats/ dir carries the SAME `sessionId` in its header. Without
    // deduping we'd hand back the same logical session multiple times,
    // which surfaces as duplicate React keys in the chat history list
    // (Phase 2.5 finding #550). Keep the entry with the latest
    // timestamp (i.e. the most recently-updated checkpoint), which
    // also happens to carry the most accurate message_count.
    use std::collections::HashMap;
    let mut by_session: HashMap<String, ChatSession> = HashMap::new();
    for s in results {
        match by_session.get(&s.session_id) {
            Some(existing) if existing.timestamp >= s.timestamp => {}
            _ => {
                by_session.insert(s.session_id.clone(), s);
            }
        }
    }
    Ok(by_session.into_values().collect())
}

// ── Pi chat parsing ─────────────────────────────────────────────────────

pub fn parse_pi_sessions(project_filter: Option<&str>) -> Result<Vec<ChatSession>, String> {
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return Ok(vec![]),
    };
    let sessions_root = home.join(".pi").join("agent").join("sessions");
    if !sessions_root.exists() {
        return Ok(vec![]);
    }
    let filter_root = project_filter.map(resolve_root_project_path);
    let slug_dirs = match fs::read_dir(&sessions_root) {
        Ok(e) => e,
        Err(_) => return Ok(vec![]),
    };
    let mut results = Vec::new();
    for slug_entry in slug_dirs.filter_map(|e| e.ok()) {
        let slug_path = slug_entry.path();
        if !slug_path.is_dir() {
            continue;
        }
        let session_files = match fs::read_dir(&slug_path) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for f_entry in session_files.filter_map(|e| e.ok()) {
            let path = f_entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
                continue;
            }
            let file = match File::open(&path) {
                Ok(f) => f,
                Err(_) => continue,
            };
            let reader = BufReader::new(file);
            let mut lines = reader.lines();
            let header_line = match lines.next() {
                Some(Ok(l)) => l,
                _ => continue,
            };
            let header: serde_json::Value = match serde_json::from_str(&header_line) {
                Ok(v) => v,
                Err(_) => continue,
            };
            if header.get("type").and_then(|v| v.as_str()) != Some("session") {
                continue;
            }
            let session_id = match header.get("id").and_then(|v| v.as_str()) {
                Some(s) => s.to_string(),
                None => continue,
            };
            let cwd = header
                .get("cwd")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if let Some(ref root) = filter_root {
                if !matches_project_family(&cwd, root) {
                    continue;
                }
            }
            let mut latest_ts_ms = parse_rfc3339_to_ms(
                header
                    .get("timestamp")
                    .and_then(|v| v.as_str())
                    .unwrap_or(""),
            )
            .unwrap_or(0);
            let mut title = String::new();
            let mut message_count: usize = 0;
            for line in lines.flatten() {
                let parsed: serde_json::Value = match serde_json::from_str(&line) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                if let Some(ts) = parsed.get("timestamp").and_then(|v| v.as_str()) {
                    if let Some(ms) = parse_rfc3339_to_ms(ts) {
                        if ms > latest_ts_ms {
                            latest_ts_ms = ms;
                        }
                    }
                }
                if parsed.get("type").and_then(|v| v.as_str()) != Some("message") {
                    continue;
                }
                let msg = match parsed.get("message") {
                    Some(m) => m,
                    None => continue,
                };
                let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("");
                if role != "user" && role != "assistant" {
                    continue;
                }
                message_count += 1;
                if title.is_empty() && role == "user" {
                    let extracted = msg.get("content").and_then(|c| c.as_array()).and_then(|arr| {
                        arr.iter().find_map(|item| {
                            if item.get("type").and_then(|v| v.as_str()) == Some("text") {
                                item.get("text").and_then(|v| v.as_str()).map(String::from)
                            } else {
                                None
                            }
                        })
                    });
                    if let Some(s) = extracted {
                        title = s.trim().to_string();
                    }
                }
            }
            let timestamp = if latest_ts_ms > 0 {
                latest_ts_ms
            } else {
                fs::metadata(&path)
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_millis() as i64)
                    .unwrap_or(0)
            };
            let truncated_title = if title.is_empty() {
                "Untitled".to_string()
            } else if title.chars().count() > 80 {
                let truncated: String = title.chars().take(77).collect();
                format!("{}...", truncated)
            } else {
                title
            };
            results.push(ChatSession {
                session_id,
                project: cwd.clone(),
                origin_branch: extract_worktree_branch(&cwd),
                title: truncated_title,
                timestamp,
                provider: "pi".to_string(),
                message_count,
            });
        }
    }
    // Dedupe by session_id: Pi's CLI checkpoints a resumed session into
    // multiple .jsonl files under different slug directories, but every
    // file carries the SAME `id` in its session header. Without this
    // pass we'd hand the same logical chat back multiple times and
    // surface duplicate React keys in the history list. Same fix
    // shape as parse_gemini_sessions (#550) and parse_cursor_sessions.
    // Keep the entry with the latest timestamp (most recent checkpoint
    // also tends to carry the highest message_count).
    use std::collections::HashMap;
    let mut by_id: HashMap<String, ChatSession> = HashMap::new();
    for s in results {
        match by_id.get(&s.session_id) {
            Some(existing) if existing.timestamp >= s.timestamp => {}
            _ => {
                by_id.insert(s.session_id.clone(), s);
            }
        }
    }
    Ok(by_id.into_values().collect())
}

// ── Codex chat parsing ──────────────────────────────────────────────────

fn codex_history_index() -> HashMap<String, (i64, String)> {
    let mut map: HashMap<String, (i64, String)> = HashMap::new();
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return map,
    };
    let path = home.join(".codex").join("history.jsonl");
    let file = match File::open(&path) {
        Ok(f) => f,
        Err(_) => return map,
    };
    for line in BufReader::new(file).lines().flatten() {
        let parsed: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let sid = match parsed.get("session_id").and_then(|v| v.as_str()) {
            Some(s) => s.to_string(),
            None => continue,
        };
        let ts = parsed.get("ts").and_then(|v| v.as_i64()).unwrap_or(0);
        let text = parsed
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        map.entry(sid)
            .and_modify(|(existing_ts, existing_text)| {
                if ts < *existing_ts || existing_text.is_empty() {
                    *existing_ts = ts;
                    *existing_text = text.clone();
                }
            })
            .or_insert((ts, text));
    }
    map
}

pub fn parse_codex_sessions(project_filter: Option<&str>) -> Result<Vec<ChatSession>, String> {
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return Ok(vec![]),
    };
    let sessions_root = home.join(".codex").join("sessions");
    if !sessions_root.exists() {
        return Ok(vec![]);
    }
    let filter_root = project_filter.map(resolve_root_project_path);
    let title_index = codex_history_index();
    let mut results = Vec::new();
    let years = match fs::read_dir(&sessions_root) {
        Ok(e) => e,
        Err(_) => return Ok(vec![]),
    };
    for year_entry in years.flatten() {
        if !year_entry.path().is_dir() {
            continue;
        }
        let months = match fs::read_dir(year_entry.path()) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for month_entry in months.flatten() {
            if !month_entry.path().is_dir() {
                continue;
            }
            let days = match fs::read_dir(month_entry.path()) {
                Ok(e) => e,
                Err(_) => continue,
            };
            for day_entry in days.flatten() {
                if !day_entry.path().is_dir() {
                    continue;
                }
                let files = match fs::read_dir(day_entry.path()) {
                    Ok(e) => e,
                    Err(_) => continue,
                };
                for f_entry in files.flatten() {
                    let path = f_entry.path();
                    if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
                        continue;
                    }
                    let file = match File::open(&path) {
                        Ok(f) => f,
                        Err(_) => continue,
                    };
                    let mut reader = BufReader::new(file);
                    let mut first_line = String::new();
                    if reader.read_line(&mut first_line).is_err() {
                        continue;
                    }
                    let header: serde_json::Value = match serde_json::from_str(first_line.trim()) {
                        Ok(v) => v,
                        Err(_) => continue,
                    };
                    if header.get("type").and_then(|v| v.as_str()) != Some("session_meta") {
                        continue;
                    }
                    let payload = match header.get("payload") {
                        Some(p) => p,
                        None => continue,
                    };
                    let session_id = match payload.get("id").and_then(|v| v.as_str()) {
                        Some(s) => s.to_string(),
                        None => continue,
                    };
                    let cwd = payload
                        .get("cwd")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    if let Some(ref root) = filter_root {
                        if !matches_project_family(&cwd, root) {
                            continue;
                        }
                    }
                    let mtime_ms = fs::metadata(&path)
                        .ok()
                        .and_then(|m| m.modified().ok())
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_millis() as i64)
                        .unwrap_or(0);
                    let header_ms = parse_rfc3339_to_ms(
                        payload
                            .get("timestamp")
                            .and_then(|v| v.as_str())
                            .unwrap_or(""),
                    )
                    .unwrap_or(0);
                    let timestamp = if mtime_ms > 0 { mtime_ms } else { header_ms };
                    let raw_title = title_index
                        .get(&session_id)
                        .map(|(_, t)| t.as_str())
                        .unwrap_or("");
                    let truncated_title = if raw_title.is_empty() {
                        format!(
                            "Codex session {}",
                            &session_id[..8.min(session_id.len())]
                        )
                    } else if raw_title.chars().count() > 80 {
                        let truncated: String = raw_title.chars().take(77).collect();
                        format!("{}...", truncated)
                    } else {
                        raw_title.to_string()
                    };
                    results.push(ChatSession {
                        session_id,
                        project: cwd.clone(),
                        origin_branch: extract_worktree_branch(&cwd),
                        title: truncated_title,
                        timestamp,
                        provider: "codex".to_string(),
                        message_count: 0,
                    });
                }
            }
        }
    }
    // Dedupe by session_id: Codex writes one .jsonl per resumed slice
    // of the same logical session, each with a fresh
    // `~/.codex/sessions/YYYY/MM/DD/...` path but the SAME `payload.id`
    // in its session_meta header. Without this pass we surface every
    // resume as a separate row in the history list (duplicate React
    // keys, inflated "session count"). Mirrors the fix in
    // parse_gemini_sessions (#550). Keep the entry with the latest
    // timestamp (last-modified file = most recent resume).
    use std::collections::HashMap;
    let mut by_id: HashMap<String, ChatSession> = HashMap::new();
    for s in results {
        match by_id.get(&s.session_id) {
            Some(existing) if existing.timestamp >= s.timestamp => {}
            _ => {
                by_id.insert(s.session_id.clone(), s);
            }
        }
    }
    Ok(by_id.into_values().collect())
}

// ── Aggregate list (claude + cursor + gemini + pi + codex) ────────────

pub fn list_all_sessions(project_filter: Option<&str>) -> Result<Vec<ChatSession>, String> {
    let mut all = parse_claude_sessions(project_filter)?;
    all.extend(parse_cursor_sessions(project_filter)?);
    all.extend(parse_gemini_sessions(project_filter)?);
    all.extend(parse_pi_sessions(project_filter)?);
    all.extend(parse_codex_sessions(project_filter)?);
    all.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    all.truncate(100);
    Ok(all)
}

// ── Storage path discovery ─────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatStoragePaths {
    pub claude_history_file: Option<String>,
    pub claude_sessions_dirs: Vec<String>,
    pub cursor_chats_dirs: Vec<String>,
    pub gemini_chats_dirs: Vec<String>,
    pub pi_chats_dirs: Vec<String>,
    pub codex_sessions_dirs: Vec<String>,
    pub codex_history_file: Option<String>,
}

pub fn get_storage_paths(project_path: &str) -> Result<ChatStoragePaths, String> {
    let home = dirs::home_dir().ok_or("Cannot determine home directory")?;
    let root = resolve_root_project_path(project_path);
    let root_hash = claude_project_hash(root);

    let claude_history_file = {
        let p = home.join(".claude").join("history.jsonl");
        if p.exists() {
            Some(p.to_string_lossy().to_string())
        } else {
            None
        }
    };
    let claude_sessions_dirs = {
        let projects_dir = home.join(".claude").join("projects");
        match fs::read_dir(&projects_dir) {
            Ok(entries) => entries
                .filter_map(|e| e.ok())
                .filter(|e| {
                    let name = e.file_name().to_string_lossy().to_string();
                    e.path().is_dir()
                        && (name == root_hash
                            || name.starts_with(&format!("{}-.worktrees-", root_hash)))
                })
                .map(|e| e.path().to_string_lossy().to_string())
                .collect(),
            Err(_) => vec![],
        }
    };
    let cursor_chats_dirs = {
        let chats_dir = home.join(".cursor").join("chats");
        match fs::read_dir(&chats_dir) {
            Ok(entries) => entries
                .filter_map(|e| e.ok())
                .filter(|e| {
                    let name = e.file_name().to_string_lossy().to_string();
                    e.path().is_dir()
                        && (name == root_hash
                            || name.starts_with(&format!("{}-.worktrees-", root_hash)))
                })
                .map(|e| e.path().to_string_lossy().to_string())
                .collect(),
            Err(_) => vec![],
        }
    };
    let gemini_chats_dirs = {
        let tmp_dir = home.join(".gemini").join("tmp");
        let slug_to_cwd = gemini_slug_to_cwd_map();
        slug_to_cwd
            .iter()
            .filter(|(_slug, cwd)| matches_project_family(cwd, root))
            .map(|(slug, _cwd)| {
                tmp_dir
                    .join(slug)
                    .join("chats")
                    .to_string_lossy()
                    .to_string()
            })
            .filter(|p| std::path::Path::new(p).is_dir())
            .collect()
    };
    let pi_chats_dirs = {
        let sessions_root = home.join(".pi").join("agent").join("sessions");
        let mut out: Vec<String> = Vec::new();
        if let Ok(entries) = fs::read_dir(&sessions_root) {
            for slug_entry in entries.filter_map(|e| e.ok()) {
                let slug_path = slug_entry.path();
                if !slug_path.is_dir() {
                    continue;
                }
                let session_files = match fs::read_dir(&slug_path) {
                    Ok(e) => e,
                    Err(_) => continue,
                };
                let mut matched = false;
                for f_entry in session_files.filter_map(|e| e.ok()) {
                    let p = f_entry.path();
                    if p.extension().and_then(|s| s.to_str()) != Some("jsonl") {
                        continue;
                    }
                    if let Ok(file) = File::open(&p) {
                        let mut reader = BufReader::new(file);
                        let mut first = String::new();
                        if reader.read_line(&mut first).is_ok() {
                            if let Ok(header) =
                                serde_json::from_str::<serde_json::Value>(first.trim())
                            {
                                if let Some(cwd) = header.get("cwd").and_then(|v| v.as_str()) {
                                    if matches_project_family(cwd, root) {
                                        matched = true;
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }
                if matched {
                    out.push(slug_path.to_string_lossy().to_string());
                }
            }
        }
        out
    };
    let codex_history_file = {
        let p = home.join(".codex").join("history.jsonl");
        if p.exists() {
            Some(p.to_string_lossy().to_string())
        } else {
            None
        }
    };
    let codex_sessions_dirs = {
        let mut out = Vec::new();
        let sessions_root = home.join(".codex").join("sessions");
        if let Ok(years) = fs::read_dir(&sessions_root) {
            for year_entry in years.filter_map(|e| e.ok()) {
                if !year_entry.path().is_dir() {
                    continue;
                }
                if let Ok(months) = fs::read_dir(year_entry.path()) {
                    for month_entry in months.filter_map(|e| e.ok()) {
                        if !month_entry.path().is_dir() {
                            continue;
                        }
                        if let Ok(days) = fs::read_dir(month_entry.path()) {
                            for day_entry in days.filter_map(|e| e.ok()) {
                                let day_path = day_entry.path();
                                if !day_path.is_dir() {
                                    continue;
                                }
                                let mut matched = false;
                                if let Ok(files) = fs::read_dir(&day_path) {
                                    for f in files.filter_map(|e| e.ok()) {
                                        let p = f.path();
                                        if p.extension().and_then(|s| s.to_str()) != Some("jsonl") {
                                            continue;
                                        }
                                        if let Ok(file) = File::open(&p) {
                                            let mut reader = BufReader::new(file);
                                            let mut first = String::new();
                                            if reader.read_line(&mut first).is_ok() {
                                                if let Ok(v) = serde_json::from_str::<
                                                    serde_json::Value,
                                                >(
                                                    first.trim()
                                                ) {
                                                    if let Some(cwd) = v
                                                        .get("payload")
                                                        .and_then(|p| p.get("cwd"))
                                                        .and_then(|v| v.as_str())
                                                    {
                                                        if matches_project_family(cwd, root) {
                                                            matched = true;
                                                            break;
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                                if matched {
                                    out.push(day_path.to_string_lossy().to_string());
                                }
                            }
                        }
                    }
                }
            }
        }
        out
    };
    Ok(ChatStoragePaths {
        claude_history_file,
        claude_sessions_dirs,
        cursor_chats_dirs,
        gemini_chats_dirs,
        pi_chats_dirs,
        codex_sessions_dirs,
        codex_history_file,
    })
}

// ── chat_session_names DB operations ───────────────────────────────────

pub fn get_custom_names() -> Result<HashMap<String, String>, String> {
    let db = crate::db::shared();
    let conn = db.lock();
    let mut stmt = conn
        .prepare("SELECT provider, session_id, custom_name FROM chat_session_names")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| {
            let provider: String = row.get(0)?;
            let session_id: String = row.get(1)?;
            let custom_name: String = row.get(2)?;
            Ok((format!("{}:{}", provider, session_id), custom_name))
        })
        .map_err(|e| e.to_string())?;
    let mut map = HashMap::new();
    for row in rows.flatten() {
        let (key, name) = row;
        map.insert(key, name);
    }
    Ok(map)
}

pub fn rename_session(
    provider: &str,
    session_id: &str,
    custom_name: &str,
) -> Result<(), String> {
    let db = crate::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO chat_session_names (provider, session_id, custom_name, pinned, updated_at) \
         VALUES (?1, ?2, ?3, 0, unixepoch()) \
         ON CONFLICT(provider, session_id) DO UPDATE SET custom_name = ?3, updated_at = unixepoch()",
        rusqlite::params![provider, session_id, custom_name],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn get_pinned() -> Result<Vec<String>, String> {
    let db = crate::db::shared();
    let conn = db.lock();
    let mut stmt = conn
        .prepare("SELECT provider, session_id FROM chat_session_names WHERE pinned = 1")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| {
            let provider: String = row.get(0)?;
            let session_id: String = row.get(1)?;
            Ok(format!("{}:{}", provider, session_id))
        })
        .map_err(|e| e.to_string())?;
    let mut result = Vec::new();
    for row in rows.flatten() {
        result.push(row);
    }
    Ok(result)
}

pub fn toggle_pin(provider: &str, session_id: &str, pinned: bool) -> Result<(), String> {
    let db = crate::db::shared();
    let conn = db.lock();
    let pinned_val: i64 = if pinned { 1 } else { 0 };
    conn.execute(
        "INSERT INTO chat_session_names (provider, session_id, custom_name, pinned, updated_at) \
         VALUES (?1, ?2, '', ?3, unixepoch()) \
         ON CONFLICT(provider, session_id) DO UPDATE SET pinned = ?3, updated_at = unixepoch()",
        rusqlite::params![provider, session_id, pinned_val],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

// ── Cursor IDE migration ───────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CursorIdeSession {
    pub composer_id: String,
    pub name: String,
    pub created_at: i64,
    pub last_updated_at: i64,
    pub mode: String,
    pub already_migrated: bool,
    pub migratable: bool,
}

/// Discover Cursor IDE sessions for a given project path that could be
/// migrated to the CLI format. Mirrors the pre-Phase-2 Tauri command.
pub fn discover_ide_sessions(project_path: &str) -> Result<Vec<CursorIdeSession>, String> {
    let home = dirs::home_dir().ok_or("Cannot determine home directory")?;
    let ws_storage = home.join("Library/Application Support/Cursor/User/workspaceStorage");
    if !ws_storage.exists() {
        return Ok(vec![]);
    }
    let cursor_chats_dir = home.join(".cursor").join("chats");
    let global_db_path =
        home.join("Library/Application Support/Cursor/User/globalStorage/state.vscdb");
    let global_conn = if global_db_path.exists() {
        rusqlite::Connection::open_with_flags(
            &global_db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .ok()
    } else {
        None
    };
    let entries = fs::read_dir(&ws_storage).map_err(|e| e.to_string())?;
    let mut results = Vec::new();
    for entry in entries.flatten() {
        let ws_path = entry.path();
        if !ws_path.is_dir() {
            continue;
        }
        let ws_json_path = ws_path.join("workspace.json");
        let ws_json = match fs::read_to_string(&ws_json_path) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let ws_data: serde_json::Value = match serde_json::from_str(&ws_json) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let folder_uri = match ws_data.get("folder").and_then(|v| v.as_str()) {
            Some(f) => f.to_string(),
            None => continue,
        };
        let folder_path = percent_decode_uri(&folder_uri);
        let root = resolve_root_project_path(project_path);
        if !matches_project_family(&folder_path, root) {
            continue;
        }
        let state_db_path = ws_path.join("state.vscdb");
        if !state_db_path.exists() {
            continue;
        }
        let conn = match rusqlite::Connection::open_with_flags(
            &state_db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        ) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let composer_json: String = match conn.query_row(
            "SELECT value FROM ItemTable WHERE key = 'composer.composerData'",
            [],
            |row| row.get(0),
        ) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let composer_data: serde_json::Value = match serde_json::from_str(&composer_json) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let composers = match composer_data.get("allComposers").and_then(|v| v.as_array()) {
            Some(arr) => arr,
            None => continue,
        };
        for composer in composers {
            let composer_id = match composer.get("composerId").and_then(|v| v.as_str()) {
                Some(id) => id.to_string(),
                None => continue,
            };
            if composer
                .get("isArchived")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
                || composer
                    .get("isDraft")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
                || composer
                    .get("isEphemeral")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
                || composer.get("createdFromBackgroundAgent").is_some()
                || composer.get("subagentInfo").is_some()
            {
                continue;
            }
            let name = composer
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("Untitled")
                .to_string();
            if name.is_empty() || name == "Untitled" {
                let headers = composer
                    .get("fullConversationHeadersOnly")
                    .and_then(|v| v.as_array())
                    .map(|a| a.len())
                    .unwrap_or(0);
                if headers == 0 {
                    continue;
                }
            }
            let created_at = composer.get("createdAt").and_then(|v| v.as_i64()).unwrap_or(0);
            let last_updated_at = composer
                .get("lastUpdatedAt")
                .and_then(|v| v.as_i64())
                .unwrap_or(created_at);
            let mode = composer
                .get("unifiedMode")
                .and_then(|v| v.as_str())
                .unwrap_or("agent")
                .to_string();
            let already_migrated = cursor_chats_dir.exists()
                && fs::read_dir(&cursor_chats_dir)
                    .ok()
                    .map(|entries| {
                        entries.filter_map(|e| e.ok()).any(|e| {
                            e.path().join(&composer_id).join("store.db").exists()
                        })
                    })
                    .unwrap_or(false);
            let migratable = if already_migrated {
                true
            } else if let Some(ref gc) = global_conn {
                let key = format!("composerData:{}", composer_id);
                gc.query_row(
                    "SELECT value FROM cursorDiskKV WHERE key = ?1",
                    rusqlite::params![key],
                    |row| row.get::<_, String>(0),
                )
                .ok()
                .and_then(|val| serde_json::from_str::<serde_json::Value>(&val).ok())
                .map(|data| {
                    let cs = data
                        .get("conversationState")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    cs.len() > 10
                })
                .unwrap_or(false)
            } else {
                false
            };
            results.push(CursorIdeSession {
                composer_id,
                name,
                created_at,
                last_updated_at,
                mode,
                already_migrated,
                migratable,
            });
        }
    }
    results.sort_by(|a, b| b.last_updated_at.cmp(&a.last_updated_at));
    Ok(results)
}

/// Migrate Cursor IDE sessions to CLI format. Creates store.db files
/// under `~/.cursor/chats/{md5(projectPath)}/{composerId}/`.
pub fn migrate_ide_sessions(
    project_path: &str,
    composer_ids: &[String],
) -> Result<usize, String> {
    let home = dirs::home_dir().ok_or("Cannot determine home directory")?;
    let ws_storage = home.join("Library/Application Support/Cursor/User/workspaceStorage");
    let global_db_path =
        home.join("Library/Application Support/Cursor/User/globalStorage/state.vscdb");
    let cursor_chats_dir = home.join(".cursor").join("chats");
    let project_md5 = md5_hex(project_path.as_bytes());
    if !ws_storage.exists() || !global_db_path.exists() {
        return Err("Cursor data not found".to_string());
    }
    let global_conn = rusqlite::Connection::open_with_flags(
        &global_db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| format!("Failed to open globalStorage: {}", e))?;
    let mut composer_data_map: HashMap<String, serde_json::Value> = HashMap::new();
    let entries = fs::read_dir(&ws_storage).map_err(|e| e.to_string())?;
    for entry in entries.flatten() {
        let ws_path = entry.path();
        if !ws_path.is_dir() {
            continue;
        }
        let ws_json_path = ws_path.join("workspace.json");
        let ws_json = match fs::read_to_string(&ws_json_path) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let ws_data: serde_json::Value = match serde_json::from_str(&ws_json) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let folder_uri = match ws_data.get("folder").and_then(|v| v.as_str()) {
            Some(f) => f,
            None => continue,
        };
        let folder_path = percent_decode_uri(folder_uri);
        let root = resolve_root_project_path(project_path);
        if !matches_project_family(&folder_path, root) {
            continue;
        }
        let state_db_path = ws_path.join("state.vscdb");
        if !state_db_path.exists() {
            continue;
        }
        let conn = match rusqlite::Connection::open_with_flags(
            &state_db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        ) {
            Ok(c) => c,
            Err(_) => continue,
        };
        for cid in composer_ids {
            let key = format!("composerData:{}", cid);
            if let Ok(value) = global_conn.query_row(
                "SELECT value FROM cursorDiskKV WHERE key = ?1",
                rusqlite::params![key],
                |row| row.get::<_, String>(0),
            ) {
                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&value) {
                    composer_data_map.insert(cid.clone(), parsed);
                }
            }
        }
        if let Ok(composer_json) = conn.query_row(
            "SELECT value FROM ItemTable WHERE key = 'composer.composerData'",
            [],
            |row| row.get::<_, String>(0),
        ) {
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&composer_json) {
                if let Some(all) = parsed.get("allComposers").and_then(|v| v.as_array()) {
                    for c in all {
                        if let Some(cid) = c.get("composerId").and_then(|v| v.as_str()) {
                            if composer_ids.contains(&cid.to_string())
                                && !composer_data_map.contains_key(cid)
                            {
                                let key = format!("composerData:{}", cid);
                                if let Ok(value) = global_conn.query_row(
                                    "SELECT value FROM cursorDiskKV WHERE key = ?1",
                                    rusqlite::params![key],
                                    |row| row.get::<_, String>(0),
                                ) {
                                    if let Ok(p) =
                                        serde_json::from_str::<serde_json::Value>(&value)
                                    {
                                        composer_data_map.insert(cid.to_string(), p);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    let mut migrated_count = 0;
    for composer_id in composer_ids {
        let data = match composer_data_map.get(composer_id) {
            Some(d) => d,
            None => continue,
        };
        let conversation_state = match data
            .get("conversationState")
            .and_then(|v| v.as_str())
        {
            Some(cs) if !cs.is_empty() => cs.to_string(),
            _ => continue,
        };
        let root_blob_data = if conversation_state.starts_with('~') {
            let cs_clean = conversation_state.trim_start_matches('~');
            let mut padded = cs_clean.to_string();
            let pad_len = (4 - padded.len() % 4) % 4;
            for _ in 0..pad_len {
                padded.push('=');
            }
            match base64_decode(&padded) {
                Some(d) if !d.is_empty() => d,
                _ => continue,
            }
        } else {
            let chars: Vec<char> = conversation_state.chars().collect();
            if chars.len() % 2 != 0 || chars.len() < 4 {
                continue;
            }
            let mut bytes = Vec::with_capacity(chars.len() / 2);
            let mut valid = true;
            for chunk in chars.chunks(2) {
                let s: String = chunk.iter().collect();
                match u8::from_str_radix(&s, 16) {
                    Ok(b) => bytes.push(b),
                    Err(_) => {
                        valid = false;
                        break;
                    }
                }
            }
            if !valid || bytes.is_empty() {
                continue;
            }
            bytes
        };
        let root_blob_id = sha256_hex(&root_blob_data);
        let mut all_blob_hashes: Vec<String> = Vec::new();
        collect_all_blob_hashes(&root_blob_data, &mut all_blob_hashes);
        let session_dir = cursor_chats_dir.join(&project_md5).join(composer_id);
        if fs::create_dir_all(&session_dir).is_err() {
            continue;
        }
        let store_db_path = session_dir.join("store.db");
        let store_conn = match rusqlite::Connection::open(&store_db_path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        store_conn
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS blobs (id TEXT PRIMARY KEY, data BLOB); \
                 CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT);",
            )
            .map_err(|e| format!("Failed to create tables: {}", e))?;
        let _ = store_conn.execute(
            "INSERT OR REPLACE INTO blobs (id, data) VALUES (?1, ?2)",
            rusqlite::params![root_blob_id, root_blob_data],
        );
        let mut copied: std::collections::HashSet<String> = std::collections::HashSet::new();
        copied.insert(root_blob_id.clone());
        let mut queue: std::collections::VecDeque<String> =
            all_blob_hashes.iter().cloned().collect();
        while let Some(hash) = queue.pop_front() {
            if copied.contains(&hash) {
                continue;
            }
            copied.insert(hash.clone());
            let key = format!("agentKv:blob:{}", hash);
            if let Ok(blob_data) = global_conn.query_row(
                "SELECT value FROM cursorDiskKV WHERE key = ?1",
                rusqlite::params![key],
                |row| row.get::<_, Vec<u8>>(0),
            ) {
                let _ = store_conn.execute(
                    "INSERT OR REPLACE INTO blobs (id, data) VALUES (?1, ?2)",
                    rusqlite::params![hash, blob_data],
                );
                let mut sub_hashes: Vec<String> = Vec::new();
                collect_all_blob_hashes(&blob_data, &mut sub_hashes);
                for sub_hash in sub_hashes {
                    if !copied.contains(&sub_hash) {
                        queue.push_back(sub_hash);
                    }
                }
            }
        }
        let name = data
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("Migrated Session");
        let created_at = data.get("createdAt").and_then(|v| v.as_i64()).unwrap_or(0);
        let _last_updated_at = data
            .get("lastUpdatedAt")
            .and_then(|v| v.as_i64())
            .unwrap_or(created_at);
        let mode = data
            .get("unifiedMode")
            .and_then(|v| v.as_str())
            .unwrap_or("default");
        let meta_str = format!(
            "{{\"agentId\":\"{}\",\"latestRootBlobId\":\"{}\",\"name\":{},\"mode\":\"{}\",\"createdAt\":{},\"lastUsedModel\":\"composer-2-fast\"}}",
            composer_id,
            root_blob_id,
            serde_json::to_string(name).unwrap_or_else(|_| "\"Migrated Session\"".to_string()),
            mode,
            created_at,
        );
        let meta_hex = string_to_hex(&meta_str);
        let _ = store_conn.execute(
            "INSERT OR REPLACE INTO meta (key, value) VALUES ('0', ?1)",
            rusqlite::params![meta_hex],
        );
        migrated_count += 1;
    }
    Ok(migrated_count)
}

// ── Hashing / encoding helpers (migrated from Tauri) ───────────────────

fn percent_decode_uri(uri: &str) -> String {
    uri.strip_prefix("file://")
        .unwrap_or(uri)
        .replace("%20", " ")
        .replace("%28", "(")
        .replace("%29", ")")
        .replace("%5B", "[")
        .replace("%5D", "]")
        .replace("%23", "#")
        .replace("%25", "%")
}

/// MD5 hash → 32-char lowercase hex.
fn md5_hex(data: &[u8]) -> String {
    let digest = md5_digest(data);
    digest.iter().map(|b| format!("{:02x}", b)).collect()
}

fn md5_digest(data: &[u8]) -> [u8; 16] {
    let mut state: [u32; 4] = [0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476];
    let s: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5,
        9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6,
        10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    let k: [u32; 64] = [
        0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613,
        0xfd469501, 0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193,
        0xa679438e, 0x49b40821, 0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d,
        0x02441453, 0xd8a1e681, 0xe7d3fbc8, 0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed,
        0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a, 0xfffa3942, 0x8771f681, 0x6d9d6122,
        0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70, 0x289b7ec6, 0xeaa127fa,
        0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665, 0xf4292244,
        0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
        0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb,
        0xeb86d391,
    ];
    let orig_len = data.len();
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    let bit_len = (orig_len as u64).wrapping_mul(8);
    msg.extend_from_slice(&bit_len.to_le_bytes());
    for chunk in msg.chunks(64) {
        let mut m = [0u32; 16];
        for (i, word) in m.iter_mut().enumerate() {
            *word = u32::from_le_bytes([
                chunk[i * 4],
                chunk[i * 4 + 1],
                chunk[i * 4 + 2],
                chunk[i * 4 + 3],
            ]);
        }
        let [mut a, mut b, mut c, mut d] = state;
        for i in 0..64 {
            let (f, g) = match i {
                0..=15 => ((b & c) | (!b & d), i),
                16..=31 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                32..=47 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let temp = d;
            d = c;
            c = b;
            b = b.wrapping_add(
                (a.wrapping_add(f).wrapping_add(k[i]).wrapping_add(m[g])).rotate_left(s[i]),
            );
            a = temp;
        }
        state[0] = state[0].wrapping_add(a);
        state[1] = state[1].wrapping_add(b);
        state[2] = state[2].wrapping_add(c);
        state[3] = state[3].wrapping_add(d);
    }
    let mut result = [0u8; 16];
    for (i, word) in state.iter().enumerate() {
        result[i * 4..i * 4 + 4].copy_from_slice(&word.to_le_bytes());
    }
    result
}

fn sha256_hex(data: &[u8]) -> String {
    sha256_digest(data)
}

fn sha256_digest(data: &[u8]) -> String {
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let k: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let orig_len = data.len();
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    let bit_len = (orig_len as u64).wrapping_mul(8);
    msg.extend_from_slice(&bit_len.to_be_bytes());
    for chunk in msg.chunks(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                chunk[i * 4],
                chunk[i * 4 + 1],
                chunk[i * 4 + 2],
                chunk[i * 4 + 3],
            ]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let temp1 = hh.wrapping_add(s1).wrapping_add(ch).wrapping_add(k[i]).wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }
    h.iter().map(|v| format!("{:08x}", v)).collect()
}

fn collect_all_blob_hashes(data: &[u8], out: &mut Vec<String>) {
    let mut i = 0;
    while i + 33 < data.len() {
        let wire_type = data[i] & 0x07;
        if wire_type == 2 && data[i + 1] == 0x20 {
            let hash = data[i + 2..i + 34]
                .iter()
                .map(|b| format!("{:02x}", b))
                .collect::<String>();
            if !out.contains(&hash) {
                out.push(hash);
            }
            i += 34;
        } else if wire_type == 2 && i + 1 < data.len() {
            let length = data[i + 1] as usize;
            if length < 128 {
                i += 2 + length;
            } else {
                i += 1;
            }
        } else if wire_type == 0 {
            i += 1;
            while i < data.len() && data[i] & 0x80 != 0 {
                i += 1;
            }
            i += 1;
        } else {
            i += 1;
        }
    }
}

fn base64_decode(input: &str) -> Option<Vec<u8>> {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = Vec::new();
    let bytes: Vec<u8> = input
        .bytes()
        .filter(|&b| b != b'\n' && b != b'\r' && b != b' ')
        .collect();
    for chunk in bytes.chunks(4) {
        if chunk.len() < 2 {
            break;
        }
        let mut buf = [0u8; 4];
        let mut count = 0;
        for (i, &byte) in chunk.iter().enumerate() {
            if byte == b'=' {
                break;
            }
            match TABLE.iter().position(|&t| t == byte) {
                Some(pos) => {
                    buf[i] = pos as u8;
                    count = i + 1;
                }
                None => return None,
            }
        }
        if count >= 2 {
            output.push((buf[0] << 2) | (buf[1] >> 4));
        }
        if count >= 3 {
            output.push((buf[1] << 4) | (buf[2] >> 2));
        }
        if count >= 4 {
            output.push((buf[2] << 6) | buf[3]);
        }
    }
    Some(output)
}

fn string_to_hex(s: &str) -> String {
    s.bytes().map(|b| format!("{:02x}", b)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_root_project_path_strips_worktree() {
        assert_eq!(resolve_root_project_path("/r/.worktrees/f-x"), "/r");
        assert_eq!(resolve_root_project_path("/r"), "/r");
        assert_eq!(resolve_root_project_path("/r/.k2so/x"), "/r/.k2so/x");
    }

    #[test]
    fn matches_project_family_covers_root_and_worktrees() {
        assert!(matches_project_family("/r", "/r"));
        assert!(matches_project_family("/r/.worktrees/a", "/r"));
        assert!(!matches_project_family("/other", "/r"));
        // Must start with the separator — `/r2` shouldn't match `/r`.
        assert!(!matches_project_family("/r2", "/r"));
    }

    #[test]
    fn claude_project_hash_handles_hidden_dirs() {
        // Non-existent path → `canonicalize` fails → literal fallback →
        // char-map. A hidden `/.k2so` segment maps `/`→`-` AND `.`→`-`,
        // producing the `--k2so` double-hyphen run (which Claude also
        // emits). This golden survives the 0.39.44 encoder rewrite (both
        // the old `/.`→`/-` rule and the new per-char rule produce `--`).
        assert_eq!(
            claude_project_hash("/Users/z/proj/.k2so/agents/a"),
            "-Users-z-proj--k2so-agents-a"
        );
        assert_eq!(claude_project_hash("/r"), "-r");
    }

    #[test]
    fn claude_project_hash_handles_spaces() {
        // Claude turns spaces in path components to hyphens. Mirror that so
        // `claude_session_file_exists` resolves the right on-disk dir for
        // workspaces like `/Users/.../Alakazam Labs/...`. (Non-existent
        // fixtures → literal fallback → ` `→`-` char-map. This golden is
        // unchanged by the 0.39.44 encoder rewrite.)
        assert_eq!(
            claude_project_hash("/Users/z/DevProjects/Alakazam Labs/K2SO"),
            "-Users-z-DevProjects-Alakazam-Labs-K2SO"
        );
        assert_eq!(
            claude_project_hash("/Users/z/Some Folder With Spaces/proj"),
            "-Users-z-Some-Folder-With-Spaces-proj"
        );
    }

    #[test]
    fn claude_project_hash_collapses_underscores_and_dots() {
        // GH#25 root cause: the pre-0.39.44 encoder PRESERVED `_` and any
        // mid-component `.`, so K2SO and Claude diverged on these paths and
        // never saw each other's sessions. Claude maps EVERY non-`[a-zA-Z0-9]`
        // char → `-`. These fixtures are non-existent → literal fallback →
        // the char-map alone must reproduce Claude's collapse.
        // Underscore (the #25 case):
        assert_eq!(
            claude_project_hash("/Users/z/proj/scout_v3"),
            "-Users-z-proj-scout-v3"
        );
        // Mid-component dot:
        assert_eq!(claude_project_hash("/Users/z/foo.bar"), "-Users-z-foo-bar");
        // Both together — mirrors the live probe SHAPE (a `_` dir with a
        // dotted child), just without the symlinked `/tmp` prefix:
        assert_eq!(
            claude_project_hash("/k2so_slug_probe_a_b/has_under_score.dotted"),
            "-k2so-slug-probe-a-b-has-under-score-dotted"
        );
    }

    #[test]
    fn claude_project_hash_canonicalizes_symlinks() {
        // Claude canonicalizes the path (resolving symlinks) BEFORE slugging,
        // so a symlinked dir and its real target must produce the SAME slug.
        // Live probe (a real, EXISTING path so canonicalize runs):
        //   /private/tmp/k2so_slug_probe_a_b/has_under_score.dotted
        //   → -private-tmp-k2so-slug-probe-a-b-has-under-score-dotted
        // We reproduce that here with a real tmp dir + a symlink to it and
        // assert both collapse identically.
        let unique = format!(
            "k2so_slug_probe_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        // Real target dir (contains `_` and a dotted child so we also cover
        // the char-map through canonicalize).
        let real = std::env::temp_dir().join(&unique).join("has_under_score.dotted");
        std::fs::create_dir_all(&real).expect("create real probe dir");
        // Symlink whose name differs from its target.
        let link = std::env::temp_dir().join(format!("{unique}_link"));
        let _ = std::fs::remove_file(&link);
        #[cfg(unix)]
        std::os::unix::fs::symlink(
            std::env::temp_dir().join(&unique),
            &link,
        )
        .expect("create symlink");

        let via_real = claude_project_hash(&real.to_string_lossy());
        // Through the symlink to the SAME dotted child.
        let via_link = claude_project_hash(
            &link.join("has_under_score.dotted").to_string_lossy(),
        );

        // canonicalize collapses both to the identical real-path slug.
        assert_eq!(
            via_real, via_link,
            "symlinked + real paths must slug identically"
        );
        // And the slug is the char-mapped CANONICAL path: it must contain the
        // real (resolved) basename chain with `_` and `.` collapsed, and must
        // NOT contain the symlink's own `_link` segment.
        assert!(
            via_real.ends_with("-has-under-score-dotted"),
            "canonical basename collapsed: {via_real}"
        );
        assert!(
            !via_link.contains("link"),
            "symlink name resolved away, not in slug: {via_link}"
        );

        // Cleanup (best-effort).
        let _ = std::fs::remove_file(&link);
        let _ = std::fs::remove_dir_all(std::env::temp_dir().join(&unique));
    }

    #[test]
    fn cursor_project_hash_strips_leading_slash() {
        assert_eq!(
            cursor_project_hash("/Users/z/DevProjects/K2SO"),
            "Users-z-DevProjects-K2SO"
        );
    }

    #[test]
    fn detect_active_session_unknown_provider_returns_none() {
        assert_eq!(detect_active_session("bogus", "/r").unwrap(), None);
    }

    // ── Phase 2 Unit 6 additions ───────────────────────────────────
    //
    // Tests for the ~1,700 LoC migrated from the Tauri command into
    // k2so-core under "Phase 2 Unit 6 — full IDE-history parsing
    // surface" earlier in this file.

    /// Each HOME-mutating test grabs this lock so cargo's parallel
    /// runner can't see another test's HOME between the set + the
    /// inner call. Shared with `themes::tests` + `skill_layers::tests`
    /// via the crate-wide `themes::HOME_LOCK` static so the lock is a
    /// true process-wide singleton, not a per-module mutex.
    use crate::themes::HOME_LOCK as UNIT6_HOME_LOCK;

    struct U6TempDir {
        path: std::path::PathBuf,
    }

    impl U6TempDir {
        fn new(label: &str) -> Self {
            let pid = std::process::id();
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let path = std::env::temp_dir().join(format!("k2so-ch-u6-{label}-{pid}-{nanos}"));
            std::fs::create_dir_all(&path).expect("create tempdir");
            Self { path }
        }
    }

    impl Drop for U6TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    struct U6HomeGuard {
        original: Option<std::ffi::OsString>,
        _tmp: U6TempDir,
    }

    impl U6HomeGuard {
        fn new(label: &str) -> Self {
            let tmp = U6TempDir::new(label);
            let original = std::env::var_os("HOME");
            std::env::set_var("HOME", &tmp.path);
            Self { original, _tmp: tmp }
        }
    }

    impl Drop for U6HomeGuard {
        fn drop(&mut self) {
            match self.original.take() {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
        }
    }

    // ── GH#24: newest-on-disk session resolution ───────────────────
    //
    // `newest_session_in_projects_dir` is the convergence helper the
    // pinned-chat resolver uses to RESUME a real session instead of
    // minting a throwaway `--session-id` (which a reused PTY never runs
    // → endless re-mint loop on remote clients). These exercise the
    // inner, HOME-free core directly.

    /// Set a file's mtime so tests can order "newest" deterministically
    /// (filesystem write order alone isn't reliable at coarse mtime
    /// granularity). Requires the file opened with write perms.
    fn set_mtime(path: &std::path::Path, t: std::time::SystemTime) {
        let f = std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .expect("open for set_mtime");
        f.set_modified(t).expect("set_modified");
    }

    #[test]
    fn newest_session_picks_latest_nonempty_and_skips_empty() {
        let tmp = U6TempDir::new("newest-session");
        let projects = tmp.path.join(".claude").join("projects");
        let hash = "-Users-z-proj-nsi";
        let dir = projects.join(hash);
        std::fs::create_dir_all(&dir).unwrap();

        let old = dir.join("old-aaaa.jsonl");
        let new = dir.join("new-bbbb.jsonl");
        let empty = dir.join("empty-cccc.jsonl");
        std::fs::write(&old, b"{\"cwd\":\"/x\"}\n").unwrap();
        std::fs::write(&new, b"{\"cwd\":\"/x\"}\n").unwrap();
        std::fs::write(&empty, b"").unwrap();

        let base = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000);
        set_mtime(&old, base);
        set_mtime(&new, base + std::time::Duration::from_secs(10));
        // `empty` is the NEWEST by mtime but 0-byte → must be ignored
        // (a never-run `--session-id` pre-allocation; `--resume` would
        // say "No conversation found").
        set_mtime(&empty, base + std::time::Duration::from_secs(20));

        assert_eq!(
            newest_session_in_projects_dir(&projects, hash).as_deref(),
            Some("new-bbbb")
        );
    }

    #[test]
    fn newest_session_includes_worktree_siblings_and_ignores_other_projects() {
        let tmp = U6TempDir::new("newest-session-wt");
        let projects = tmp.path.join(".claude").join("projects");
        let hash = "-Users-z-proj-nsi";
        let wt_dir = projects.join(format!("{hash}-feature-x"));
        let other_dir = projects.join("-Users-z-proj-OTHER");
        std::fs::create_dir_all(&wt_dir).unwrap();
        std::fs::create_dir_all(&other_dir).unwrap();

        // A worktree-sibling session (older) and an UNRELATED project's
        // session (newest overall — must NOT be picked).
        let wt = wt_dir.join("wt-1111.jsonl");
        let other = other_dir.join("other-9999.jsonl");
        std::fs::write(&wt, b"{\"cwd\":\"/x\"}\n").unwrap();
        std::fs::write(&other, b"{\"cwd\":\"/y\"}\n").unwrap();
        let base = std::time::UNIX_EPOCH + std::time::Duration::from_secs(2_000_000);
        set_mtime(&wt, base);
        set_mtime(&other, base + std::time::Duration::from_secs(99));

        assert_eq!(
            newest_session_in_projects_dir(&projects, hash).as_deref(),
            Some("wt-1111")
        );
    }

    #[test]
    fn newest_session_none_for_brand_new_workspace() {
        let tmp = U6TempDir::new("newest-session-empty");
        let projects = tmp.path.join(".claude").join("projects");
        std::fs::create_dir_all(&projects).unwrap();
        // No slug dir for this hash at all → genuinely brand-new → None
        // (the resolver then mints a fresh `--session-id`, preserving the
        // brand-new-workspace behavior from #681).
        assert_eq!(
            newest_session_in_projects_dir(&projects, "-Users-z-proj-nsi"),
            None
        );
    }

    #[test]
    fn extract_worktree_branch_picks_up_branch_suffix() {
        assert_eq!(
            extract_worktree_branch("/repo/.worktrees/feature-x"),
            Some("feature-x".to_string())
        );
        assert_eq!(extract_worktree_branch("/repo"), None);
        assert_eq!(extract_worktree_branch(""), None);
    }

    #[test]
    fn parse_claude_sessions_returns_empty_when_history_file_missing() {
        let _g = UNIT6_HOME_LOCK.lock();
        let _h = U6HomeGuard::new("claude-empty");
        // fresh HOME → no ~/.claude/history.jsonl → empty Vec, not Err
        let sessions = parse_claude_sessions(None).expect("must not error");
        assert!(sessions.is_empty(), "got: {sessions:?}");
    }

    #[test]
    fn parse_claude_sessions_parses_seeded_history() {
        let _g = UNIT6_HOME_LOCK.lock();
        let _h = U6HomeGuard::new("claude-seeded");
        // Build ~/.claude/history.jsonl with two entries — same
        // session_id repeated to exercise the SessionAccumulator
        // collapse logic.
        let claude_dir = dirs::home_dir().unwrap().join(".claude");
        std::fs::create_dir_all(&claude_dir).unwrap();
        let history = "\
{\"sessionId\":\"s1\",\"project\":\"/repo\",\"display\":\"first prompt\",\"timestamp\":100}
{\"sessionId\":\"s1\",\"project\":\"/repo\",\"display\":\"second prompt\",\"timestamp\":200}
{\"sessionId\":\"s2\",\"project\":\"/other\",\"display\":\"only one\",\"timestamp\":50}
";
        std::fs::write(claude_dir.join("history.jsonl"), history).unwrap();
        let mut sessions = parse_claude_sessions(None).expect("parse");
        sessions.sort_by(|a, b| a.session_id.cmp(&b.session_id));
        assert_eq!(sessions.len(), 2, "got: {sessions:?}");
        let s1 = sessions.iter().find(|s| s.session_id == "s1").unwrap();
        assert_eq!(s1.message_count, 2, "two entries collapse into one");
        assert_eq!(s1.timestamp, 200, "last_timestamp wins");
        assert_eq!(s1.title, "first prompt", "first_display wins (lowest ts)");
        assert_eq!(s1.project, "/repo");
        assert_eq!(s1.provider, "claude");
    }

    #[test]
    fn parse_claude_sessions_filters_by_project() {
        let _g = UNIT6_HOME_LOCK.lock();
        let _h = U6HomeGuard::new("claude-filter");
        let claude_dir = dirs::home_dir().unwrap().join(".claude");
        std::fs::create_dir_all(&claude_dir).unwrap();
        let history = "\
{\"sessionId\":\"a\",\"project\":\"/repo\",\"display\":\"x\",\"timestamp\":1}
{\"sessionId\":\"b\",\"project\":\"/repo/.worktrees/feature\",\"display\":\"y\",\"timestamp\":2}
{\"sessionId\":\"c\",\"project\":\"/other\",\"display\":\"z\",\"timestamp\":3}
";
        std::fs::write(claude_dir.join("history.jsonl"), history).unwrap();
        let sessions = parse_claude_sessions(Some("/repo")).expect("parse");
        // /repo and /repo/.worktrees/feature both match the project
        // family — should yield 2 sessions, not the /other one.
        assert_eq!(sessions.len(), 2, "got: {sessions:?}");
        assert!(sessions.iter().any(|s| s.session_id == "a"));
        assert!(sessions.iter().any(|s| s.session_id == "b"));
        // The worktree session carries an origin_branch tag.
        let b = sessions.iter().find(|s| s.session_id == "b").unwrap();
        assert_eq!(b.origin_branch.as_deref(), Some("feature"));
    }

    #[test]
    fn parse_cursor_sessions_returns_empty_when_chats_dir_missing() {
        let _g = UNIT6_HOME_LOCK.lock();
        let _h = U6HomeGuard::new("cursor-empty");
        let sessions = parse_cursor_sessions(None).expect("must not error");
        assert!(sessions.is_empty());
    }

    #[test]
    fn parse_cursor_ide_sessions_returns_empty_when_workspace_storage_missing() {
        let _g = UNIT6_HOME_LOCK.lock();
        let _h = U6HomeGuard::new("cursor-ide-empty");
        let sessions = parse_cursor_ide_sessions(None).expect("must not error");
        assert!(sessions.is_empty());
    }

    #[test]
    fn parse_gemini_sessions_returns_empty_when_tmp_dir_missing() {
        let _g = UNIT6_HOME_LOCK.lock();
        let _h = U6HomeGuard::new("gemini-empty");
        let sessions = parse_gemini_sessions(None).expect("must not error");
        assert!(sessions.is_empty());
    }

    #[test]
    fn parse_codex_sessions_returns_empty_when_sessions_dir_missing() {
        let _g = UNIT6_HOME_LOCK.lock();
        let _h = U6HomeGuard::new("codex-empty");
        let sessions = parse_codex_sessions(None).expect("must not error");
        assert!(sessions.is_empty());
    }

    #[test]
    fn list_all_sessions_returns_empty_for_clean_home() {
        let _g = UNIT6_HOME_LOCK.lock();
        let _h = U6HomeGuard::new("all-empty");
        let sessions = list_all_sessions(None).expect("must not error");
        assert!(
            sessions.is_empty(),
            "no provider data → empty; got: {sessions:?}"
        );
    }

    #[test]
    fn get_storage_paths_reports_none_for_clean_home() {
        let _g = UNIT6_HOME_LOCK.lock();
        let _h = U6HomeGuard::new("storage-empty");
        let paths = get_storage_paths("/some/project").expect("get_storage_paths");
        assert!(paths.claude_history_file.is_none(), "got: {paths:?}");
        assert!(paths.codex_history_file.is_none());
        assert!(paths.claude_sessions_dirs.is_empty());
        assert!(paths.cursor_chats_dirs.is_empty());
        assert!(paths.gemini_chats_dirs.is_empty());
        assert!(paths.pi_chats_dirs.is_empty());
        assert!(paths.codex_sessions_dirs.is_empty());
    }

    #[test]
    fn get_storage_paths_picks_up_claude_history_file_when_present() {
        let _g = UNIT6_HOME_LOCK.lock();
        let _h = U6HomeGuard::new("storage-claude");
        let claude_dir = dirs::home_dir().unwrap().join(".claude");
        std::fs::create_dir_all(&claude_dir).unwrap();
        std::fs::write(claude_dir.join("history.jsonl"), "").unwrap();
        let paths = get_storage_paths("/proj").expect("get_storage_paths");
        assert!(paths.claude_history_file.is_some(), "got: {paths:?}");
        assert!(
            paths
                .claude_history_file
                .as_ref()
                .unwrap()
                .ends_with(".claude/history.jsonl")
        );
    }

    #[test]
    fn discover_ide_sessions_returns_empty_when_cursor_workspace_storage_missing() {
        let _g = UNIT6_HOME_LOCK.lock();
        let _h = U6HomeGuard::new("ide-empty");
        let sessions = discover_ide_sessions("/proj").expect("must not error");
        assert!(sessions.is_empty());
    }

    // ── DB-backed custom names + pin (chat_session_names table) ────
    //
    // The in-memory test DB is shared across the whole test binary, so
    // we use unique session-ID prefixes to keep these from colliding
    // with each other. Lock is intentionally separate from the HOME
    // lock — these tests don't mutate HOME.

    static UNIT6_DB_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

    #[test]
    fn rename_session_and_get_custom_names_round_trip() {
        let _g = UNIT6_DB_LOCK.lock();
        let sid = format!("u6-rn-{}-{}", std::process::id(), uuid::Uuid::new_v4());
        rename_session("claude", &sid, "My Pinned Chat").expect("rename");
        let names = get_custom_names().expect("get_custom_names");
        let key = format!("claude:{}", sid);
        assert_eq!(names.get(&key).map(|s| s.as_str()), Some("My Pinned Chat"));
        // ON CONFLICT updates the existing row → not a new one.
        rename_session("claude", &sid, "Renamed Again").expect("rename 2");
        let names2 = get_custom_names().expect("get_custom_names 2");
        assert_eq!(names2.get(&key).map(|s| s.as_str()), Some("Renamed Again"));
    }

    #[test]
    fn toggle_pin_writes_and_clears_pinned_flag() {
        let _g = UNIT6_DB_LOCK.lock();
        let sid = format!("u6-pin-{}-{}", std::process::id(), uuid::Uuid::new_v4());
        let key = format!("cursor:{}", sid);
        // Pin it.
        toggle_pin("cursor", &sid, true).expect("pin");
        let pinned = get_pinned().expect("get_pinned");
        assert!(pinned.iter().any(|k| k == &key), "got: {pinned:?}");
        // Unpin it.
        toggle_pin("cursor", &sid, false).expect("unpin");
        let pinned2 = get_pinned().expect("get_pinned 2");
        assert!(
            !pinned2.iter().any(|k| k == &key),
            "still pinned after unpin: {pinned2:?}"
        );
    }

    // ── Cross-provider dedup tests (closes #551 / audit #555 gap) ──
    //
    // Phase 2.5 fix #550 added a dedupe pass to `parse_gemini_sessions`
    // after observing that the Gemini CLI checkpoints a single logical
    // session into multiple `.jsonl` files (every file in the chats/
    // dir carries the SAME `sessionId` header). Without dedup the
    // history list surfaces duplicate React keys.
    //
    // The Cursor parser carries an equivalent `best_by_id` collapse
    // (see line ~906: `match best_by_id.get(&chat_id) { ... }`). This
    // test exercises that path with the same chat_id appearing under
    // two different project-hash directories.
    //
    // 0.39.0 follow-up: Pi, Codex, and Cursor IDE parsers all received
    // the same `HashMap<session_id, ChatSession>` dedup tail. Each
    // provider has its own quirk that produces dupes — Pi/Codex
    // checkpoint resumed sessions into multiple files, Cursor IDE can
    // mirror a composer across workspaceStorage dirs. Tests below cover
    // all three.

    #[test]
    fn parse_cursor_sessions_dedupes_chat_id_across_hash_dirs() {
        let _g = UNIT6_HOME_LOCK.lock();
        let _h = U6HomeGuard::new("cursor-dedup");
        let cursor_chats = dirs::home_dir().unwrap().join(".cursor").join("chats");
        // Two project-hash dirs, same chat_id under each.
        let hash_a = cursor_chats.join("00000000000000000000000000000001");
        let hash_b = cursor_chats.join("00000000000000000000000000000002");
        let chat_id = "shared-chat-id-xyz";
        let chat_a = hash_a.join(chat_id);
        let chat_b = hash_b.join(chat_id);
        std::fs::create_dir_all(&chat_a).unwrap();
        std::fs::create_dir_all(&chat_b).unwrap();
        // Empty (non-sqlite) store.db files trigger the file_ts
        // fallback path (read_cursor_chat_meta returns None →
        // generic "Cursor session ..." title). The parser still
        // includes them in best_by_id, which is exactly what we
        // need to exercise the dedup branch.
        std::fs::write(chat_a.join("store.db"), b"").unwrap();
        std::fs::write(chat_b.join("store.db"), b"").unwrap();

        let sessions = parse_cursor_sessions(None).expect("parse");
        // Same chat_id under two hash dirs → 1 session in output.
        let matching: Vec<_> = sessions
            .iter()
            .filter(|s| s.session_id == chat_id)
            .collect();
        assert_eq!(
            matching.len(),
            1,
            "expected dedup by chat_id; got: {sessions:?}"
        );
        assert_eq!(matching[0].provider, "cursor");
    }

    #[test]
    fn parse_pi_sessions_dedupes_session_id_across_slug_dirs() {
        // Pi checkpoints a resumed session into a fresh .jsonl under a
        // (potentially) different slug directory, but the header `id`
        // is the same logical session. Without dedup we'd surface the
        // same session twice. Build two slug dirs each containing a
        // .jsonl with the same session id and an early vs. late
        // timestamp — the late one must win.
        let _g = UNIT6_HOME_LOCK.lock();
        let _h = U6HomeGuard::new("pi-dedup");
        let sessions_root = dirs::home_dir()
            .unwrap()
            .join(".pi")
            .join("agent")
            .join("sessions");
        let slug_a = sessions_root.join("slug-a");
        let slug_b = sessions_root.join("slug-b");
        std::fs::create_dir_all(&slug_a).unwrap();
        std::fs::create_dir_all(&slug_b).unwrap();

        let session_id = "pi-shared-session-zzz";
        // Earlier checkpoint under slug-a (timestamp 2026-01-01).
        let early = format!(
            "{{\"type\":\"session\",\"id\":\"{session_id}\",\"cwd\":\"/repo\",\"timestamp\":\"2026-01-01T00:00:00Z\"}}\n\
             {{\"type\":\"message\",\"timestamp\":\"2026-01-01T00:00:01Z\",\"message\":{{\"role\":\"user\",\"content\":[{{\"type\":\"text\",\"text\":\"first\"}}]}}}}\n"
        );
        std::fs::write(slug_a.join("checkpoint-1.jsonl"), early).unwrap();

        // Later checkpoint under slug-b (timestamp 2026-05-01).
        let late = format!(
            "{{\"type\":\"session\",\"id\":\"{session_id}\",\"cwd\":\"/repo\",\"timestamp\":\"2026-05-01T00:00:00Z\"}}\n\
             {{\"type\":\"message\",\"timestamp\":\"2026-05-01T00:00:01Z\",\"message\":{{\"role\":\"user\",\"content\":[{{\"type\":\"text\",\"text\":\"second resume\"}}]}}}}\n"
        );
        std::fs::write(slug_b.join("checkpoint-2.jsonl"), late).unwrap();

        let sessions = parse_pi_sessions(None).expect("parse");
        let matching: Vec<_> = sessions
            .iter()
            .filter(|s| s.session_id == session_id)
            .collect();
        assert_eq!(
            matching.len(),
            1,
            "expected dedup by session_id across slug dirs; got: {sessions:?}",
        );
        assert_eq!(matching[0].provider, "pi");
        // Latest checkpoint wins → its title is "second resume".
        assert_eq!(
            matching[0].title, "second resume",
            "latest-timestamp entry must win",
        );
    }

    #[test]
    fn parse_codex_sessions_dedupes_session_id_across_day_dirs() {
        // Codex stores each resume slice under a fresh
        // YYYY/MM/DD/...jsonl path, but the session_meta header carries
        // the SAME `payload.id`. Without dedup the history list shows
        // one row per resume.
        let _g = UNIT6_HOME_LOCK.lock();
        let _h = U6HomeGuard::new("codex-dedup");
        let sessions_root = dirs::home_dir().unwrap().join(".codex").join("sessions");
        let day_a = sessions_root.join("2026").join("01").join("01");
        let day_b = sessions_root.join("2026").join("05").join("01");
        std::fs::create_dir_all(&day_a).unwrap();
        std::fs::create_dir_all(&day_b).unwrap();

        let session_id = "codex-shared-session-abc12345";
        let header_early = format!(
            "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"{session_id}\",\"cwd\":\"/repo\",\"timestamp\":\"2026-01-01T00:00:00Z\"}}}}\n"
        );
        let header_late = format!(
            "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"{session_id}\",\"cwd\":\"/repo\",\"timestamp\":\"2026-05-01T00:00:00Z\"}}}}\n"
        );

        // Write the two .jsonl files with naturally-staggered mtimes —
        // the parser keys `timestamp` on file mtime when present, so as
        // long as `late.mtime > early.mtime` the dedup tail keeps the
        // later one. Sleeping between writes is enough on every filesystem
        // we ship to (APFS / ext4 / btrfs all carry sub-second mtime).
        let path_early = day_a.join("rollout-early.jsonl");
        let path_late = day_b.join("rollout-late.jsonl");
        std::fs::write(&path_early, header_early).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&path_late, header_late).unwrap();

        let sessions = parse_codex_sessions(None).expect("parse");
        let matching: Vec<_> = sessions
            .iter()
            .filter(|s| s.session_id == session_id)
            .collect();
        assert_eq!(
            matching.len(),
            1,
            "expected dedup by session_id across day dirs; got: {sessions:?}",
        );
        assert_eq!(matching[0].provider, "codex");
    }

    #[test]
    fn parse_cursor_ide_sessions_dedupes_composer_id_across_workspaces() {
        // A single Cursor IDE composer can appear in multiple
        // workspaceStorage directories — e.g. when Cursor migrates
        // state between workspace IDs or the user opens the same
        // project from two different folder URIs. The state.vscdb
        // SQLite blob is the source of truth; we build two minimal
        // ones with the same composerId but different lastUpdatedAt
        // values to confirm dedup keeps the latest.
        let _g = UNIT6_HOME_LOCK.lock();
        let _h = U6HomeGuard::new("cursor-ide-dedup");
        let workspace_dir = dirs::home_dir()
            .unwrap()
            .join("Library/Application Support/Cursor/User/workspaceStorage");
        let ws_a = workspace_dir.join("ws-a");
        let ws_b = workspace_dir.join("ws-b");
        std::fs::create_dir_all(&ws_a).unwrap();
        std::fs::create_dir_all(&ws_b).unwrap();

        // Both workspaces point at the same folder URI so neither
        // gets filtered (filter is None anyway, but we still need
        // valid JSON for the parser to read past workspace.json).
        let ws_json = r#"{"folder":"file:///repo"}"#;
        std::fs::write(ws_a.join("workspace.json"), ws_json).unwrap();
        std::fs::write(ws_b.join("workspace.json"), ws_json).unwrap();

        let composer_id = "ide-shared-composer-xyz";
        // Helper: build a state.vscdb with one composer row.
        let seed_db = |path: &std::path::Path, name: &str, last_updated: i64| {
            let conn = rusqlite::Connection::open(path).expect("open state.vscdb");
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS ItemTable (key TEXT PRIMARY KEY, value TEXT);",
            )
            .expect("create ItemTable");
            let composer_json = format!(
                r#"{{"allComposers":[{{"composerId":"{composer_id}","name":"{name}","lastUpdatedAt":{last_updated},"createdAt":1000}}]}}"#
            );
            conn.execute(
                "INSERT INTO ItemTable (key, value) VALUES ('composer.composerData', ?1)",
                rusqlite::params![composer_json],
            )
            .expect("insert composer row");
        };

        seed_db(&ws_a.join("state.vscdb"), "Older Name", 1_000_000);
        seed_db(&ws_b.join("state.vscdb"), "Newer Name", 2_000_000);

        let sessions = parse_cursor_ide_sessions(None).expect("parse");
        let matching: Vec<_> = sessions
            .iter()
            .filter(|s| s.session_id == composer_id)
            .collect();
        assert_eq!(
            matching.len(),
            1,
            "expected dedup by composer_id; got: {sessions:?}",
        );
        assert_eq!(matching[0].provider, "cursor");
        // Latest lastUpdatedAt wins → its name was "Newer Name".
        assert_eq!(
            matching[0].title, "Newer Name",
            "latest-lastUpdatedAt entry must win",
        );
    }
}
