//! User-initiated chat session archive / restore (P0: Claude only).
//!
//! Distinct from [`crate::session_archive`] (protective **COPY** sweep that
//! leaves originals in place so `--resume` keeps working). This module
//! performs a **physical MOVE** of Claude session files into
//! `<project>/.k2/session-archive/user/claude/` so the conversation leaves
//! the live list and only reappears after restore.
//!
//! Soft-archive: when the live `.jsonl` is already gone (history ghosts),
//! we only flip DB flags — no move.
//!
//! See PRD: chat-session-user-archive-v1.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::chat_history::{
    claude_project_hash, matches_project_family, resolve_claude_session_file,
    resolve_root_project_path,
};

/// Snapshot of an archived chat_session_names row used by dual-query list.
#[derive(Debug, Clone)]
pub struct ArchivedSessionMeta {
    pub provider: String,
    pub session_id: String,
    pub archived_at: Option<i64>,
    pub archive_project_path: Option<String>,
    pub archive_title: Option<String>,
    pub archive_timestamp: Option<i64>,
    pub archive_source_path: Option<String>,
}

/// `<project>/.k2/session-archive/user/<provider>/`
pub fn user_archive_dir(project_path: &str, provider: &str) -> PathBuf {
    PathBuf::from(project_path)
        .join(".k2")
        .join("session-archive")
        .join("user")
        .join(provider)
}

/// Dest paths for a Claude user-archive: `.jsonl` + optional `.meta.json`.
pub fn user_archive_claude_paths(project_path: &str, session_id: &str) -> (PathBuf, PathBuf) {
    let dir = user_archive_dir(project_path, "claude");
    (
        dir.join(format!("{session_id}.jsonl")),
        dir.join(format!("{session_id}.meta.json")),
    )
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Move `src` → `dest`, creating parent dirs. Cross-device fallback: copy,
/// verify size, then delete source. Returns Ok after dest exists and src
/// is gone (or was already absent after a prior partial copy).
fn move_file(src: &Path, dest: &Path) -> Result<(), String> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }
    if !src.exists() {
        return Err(format!("source missing: {}", src.display()));
    }
    match fs::rename(src, dest) {
        Ok(()) => Ok(()),
        Err(e) => {
            // EXDEV / cross-device: copy + verify + delete
            let src_meta = fs::metadata(src)
                .map_err(|e2| format!("stat source {}: {e2}", src.display()))?;
            let src_len = src_meta.len();
            {
                let mut in_f = fs::File::open(src)
                    .map_err(|e2| format!("open source {}: {e2}", src.display()))?;
                let mut out_f = fs::File::create(dest)
                    .map_err(|e2| format!("create dest {}: {e2}", dest.display()))?;
                let mut buf = Vec::new();
                in_f
                    .read_to_end(&mut buf)
                    .map_err(|e2| format!("read source {}: {e2}", src.display()))?;
                out_f
                    .write_all(&buf)
                    .map_err(|e2| format!("write dest {}: {e2}", dest.display()))?;
                out_f
                    .sync_all()
                    .map_err(|e2| format!("sync dest {}: {e2}", dest.display()))?;
            }
            let dest_len = fs::metadata(dest)
                .map_err(|e2| format!("stat dest {}: {e2}", dest.display()))?
                .len();
            if dest_len != src_len {
                let _ = fs::remove_file(dest);
                return Err(format!(
                    "cross-device copy size mismatch ({src_len} → {dest_len}); rename err was: {e}"
                ));
            }
            fs::remove_file(src)
                .map_err(|e2| format!("delete source after copy {}: {e2}", src.display()))?;
            Ok(())
        }
    }
}

/// Upsert archive columns without clobbering custom_name / pinned.
/// Matches toggle_pin INSERT pattern (empty custom_name on insert-only).
pub fn set_archived_flags(
    provider: &str,
    session_id: &str,
    archived: bool,
    archived_at: Option<i64>,
    archive_project_path: Option<&str>,
    archive_title: Option<&str>,
    archive_timestamp: Option<i64>,
    archive_source_path: Option<&str>,
) -> Result<(), String> {
    let db = crate::db::shared();
    let conn = db.lock();
    let archived_val: i64 = if archived { 1 } else { 0 };
    if archived {
        conn.execute(
            "INSERT INTO chat_session_names \
             (provider, session_id, custom_name, pinned, updated_at, \
              archived, archived_at, archive_project_path, archive_title, \
              archive_timestamp, archive_source_path) \
             VALUES (?1, ?2, '', 0, unixepoch(), ?3, ?4, ?5, ?6, ?7, ?8) \
             ON CONFLICT(provider, session_id) DO UPDATE SET \
               archived = ?3, \
               archived_at = ?4, \
               archive_project_path = ?5, \
               archive_title = ?6, \
               archive_timestamp = ?7, \
               archive_source_path = ?8, \
               updated_at = unixepoch()",
            rusqlite::params![
                provider,
                session_id,
                archived_val,
                archived_at,
                archive_project_path,
                archive_title,
                archive_timestamp,
                archive_source_path,
            ],
        )
        .map_err(|e| e.to_string())?;
    } else {
        // Clear archive flags; leave custom_name/pinned alone.
        conn.execute(
            "INSERT INTO chat_session_names \
             (provider, session_id, custom_name, pinned, updated_at, \
              archived, archived_at, archive_project_path, archive_title, \
              archive_timestamp, archive_source_path) \
             VALUES (?1, ?2, '', 0, unixepoch(), 0, NULL, NULL, NULL, NULL, NULL) \
             ON CONFLICT(provider, session_id) DO UPDATE SET \
               archived = 0, \
               archived_at = NULL, \
               archive_project_path = NULL, \
               archive_title = NULL, \
               archive_timestamp = NULL, \
               archive_source_path = NULL, \
               updated_at = unixepoch()",
            rusqlite::params![provider, session_id],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// True if DB marks this session archived.
pub fn is_archived(provider: &str, session_id: &str) -> bool {
    let db = crate::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT archived FROM chat_session_names \
         WHERE provider = ?1 AND session_id = ?2",
        rusqlite::params![provider, session_id],
        |row| row.get::<_, i64>(0),
    )
    .map(|v| v != 0)
    .unwrap_or(false)
}

/// All archived rows, optionally filtered to a project family.
pub fn list_archived_rows(
    project_filter: Option<&str>,
) -> Result<Vec<ArchivedSessionMeta>, String> {
    let db = crate::db::shared();
    let conn = db.lock();
    let mut stmt = conn
        .prepare(
            "SELECT provider, session_id, archived_at, archive_project_path, \
                    archive_title, archive_timestamp, archive_source_path \
             FROM chat_session_names WHERE archived = 1",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| {
            Ok(ArchivedSessionMeta {
                provider: row.get(0)?,
                session_id: row.get(1)?,
                archived_at: row.get(2)?,
                archive_project_path: row.get(3)?,
                archive_title: row.get(4)?,
                archive_timestamp: row.get(5)?,
                archive_source_path: row.get(6)?,
            })
        })
        .map_err(|e| e.to_string())?;

    let root = project_filter.map(resolve_root_project_path);
    let mut out = Vec::new();
    for row in rows.flatten() {
        if let Some(root) = root {
            let proj = row.archive_project_path.as_deref().unwrap_or("");
            // Empty snapshots can't be attributed to a workspace.
            if proj.is_empty() {
                continue;
            }
            // Family match: stored path is the root itself or a worktree under it.
            // Also accept when stored path's own root equals the filter root
            // (covers archive from main while listing a worktree and vice versa).
            let same_family = matches_project_family(proj, root)
                || resolve_root_project_path(proj) == root;
            if !same_family {
                continue;
            }
        }
        out.push(row);
    }
    Ok(out)
}

/// Archive a Claude session: physical MOVE of jsonl (+ meta if present)
/// into the project's user archive dir, or soft-archive if the live file
/// is already gone. Upserts DB archive flags.
pub fn archive_user_session(
    project_path: &str,
    provider: &str,
    session_id: &str,
    title: &str,
    timestamp: i64,
) -> Result<(), String> {
    if provider != "claude" {
        return Err("only claude supported for physical archive in v1".into());
    }
    if session_id.is_empty() {
        return Err("session_id required".into());
    }
    if project_path.is_empty() {
        return Err("project_path required".into());
    }

    let (dest_jsonl, dest_meta) = user_archive_claude_paths(project_path, session_id);
    let mut source_path: Option<String> = None;

    if let Some(live) = resolve_claude_session_file(session_id, project_path) {
        source_path = Some(live.to_string_lossy().into_owned());
        move_file(&live, &dest_jsonl)?;
        // Sibling .meta.json if present.
        let live_meta = live.with_file_name(format!("{session_id}.meta.json"));
        if live_meta.exists() {
            let _ = move_file(&live_meta, &dest_meta);
        }
    } else if dest_jsonl.exists() {
        // Already in user archive (re-archive no-op for files).
        source_path = None;
    }
    // else: soft-archive only (history ghost — no live file).

    let archived_at = now_ms();
    set_archived_flags(
        provider,
        session_id,
        true,
        Some(archived_at),
        Some(project_path),
        Some(title),
        Some(timestamp),
        source_path.as_deref(),
    )?;
    Ok(())
}

/// Primary Claude projects dir for a workspace: `~/.claude/projects/<hash>/`.
fn primary_claude_project_dir(project_path: &str) -> Result<PathBuf, String> {
    let home = dirs::home_dir().ok_or_else(|| "Cannot determine home directory".to_string())?;
    let hash = claude_project_hash(resolve_root_project_path(project_path));
    Ok(home.join(".claude").join("projects").join(hash))
}

/// Is `parent` a plausible Claude projects dir for this project family?
fn source_parent_valid_for_project(parent: &Path, project_path: &str) -> bool {
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return false,
    };
    let projects = home.join(".claude").join("projects");
    let Ok(parent_canon) = parent.canonicalize() else {
        // Parent may not exist yet — still accept if it sits under the hash prefix.
        let s = parent.to_string_lossy();
        let hash = claude_project_hash(resolve_root_project_path(project_path));
        return s.contains(&hash);
    };
    let Ok(projects_canon) = projects.canonicalize() else {
        return false;
    };
    if !parent_canon.starts_with(&projects_canon) {
        return false;
    }
    let hash = claude_project_hash(resolve_root_project_path(project_path));
    parent_canon
        .file_name()
        .and_then(|n| n.to_str())
        .map(|name| name == hash || name.starts_with(&format!("{hash}-")))
        .unwrap_or(false)
}

/// Restore a Claude session from the user archive back to live storage.
pub fn restore_user_session(
    project_path: &str,
    provider: &str,
    session_id: &str,
) -> Result<(), String> {
    if provider != "claude" {
        return Err("only claude supported for physical archive in v1".into());
    }
    if session_id.is_empty() {
        return Err("session_id required".into());
    }
    if project_path.is_empty() {
        return Err("project_path required".into());
    }

    let (arch_jsonl, arch_meta) = user_archive_claude_paths(project_path, session_id);

    // Load stored source path from DB (if any).
    let stored_source: Option<String> = {
        let db = crate::db::shared();
        let conn = db.lock();
        conn.query_row(
            "SELECT archive_source_path FROM chat_session_names \
             WHERE provider = ?1 AND session_id = ?2 AND archived = 1",
            rusqlite::params![provider, session_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .unwrap_or(None)
    };

    // Soft-archived (no file): just clear flags.
    if !arch_jsonl.exists() {
        if !is_archived(provider, session_id) {
            return Err(format!(
                "archived session not found: {provider}:{session_id}"
            ));
        }
        set_archived_flags(provider, session_id, false, None, None, None, None, None)?;
        return Ok(());
    }

    let dest = if let Some(ref src) = stored_source {
        let src_path = PathBuf::from(src);
        let parent = src_path.parent().map(Path::to_path_buf);
        match parent {
            Some(p) if source_parent_valid_for_project(&p, project_path) => {
                p.join(format!("{session_id}.jsonl"))
            }
            _ => primary_claude_project_dir(project_path)?.join(format!("{session_id}.jsonl")),
        }
    } else {
        primary_claude_project_dir(project_path)?.join(format!("{session_id}.jsonl"))
    };

    move_file(&arch_jsonl, &dest)?;
    if arch_meta.exists() {
        let dest_meta = dest.with_file_name(format!("{session_id}.meta.json"));
        let _ = move_file(&arch_meta, &dest_meta);
    }

    set_archived_flags(provider, session_id, false, None, None, None, None, None)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat_history::{list_all_sessions, resolve_claude_session_file};
    use crate::themes::HOME_LOCK;
    use std::sync::Mutex;

    // Serialize DB-touching tests against the shared in-memory DB.
    static ARCHIVE_DB_LOCK: Mutex<()> = Mutex::new(());

    struct TempDir {
        path: PathBuf,
    }
    impl TempDir {
        fn new(label: &str) -> Self {
            let pid = std::process::id();
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let path = std::env::temp_dir().join(format!("k2so-ua-{label}-{pid}-{nanos}"));
            fs::create_dir_all(&path).expect("create tempdir");
            Self { path }
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    struct HomeGuard {
        original: Option<std::ffi::OsString>,
        _tmp: TempDir,
    }
    impl HomeGuard {
        fn new(label: &str) -> Self {
            let tmp = TempDir::new(label);
            let original = std::env::var_os("HOME");
            // Tests hold HOME_LOCK for exclusive mutation of process env.
            std::env::set_var("HOME", &tmp.path);
            Self {
                original,
                _tmp: tmp,
            }
        }
        fn path(&self) -> &Path {
            &self._tmp.path
        }
    }
    impl Drop for HomeGuard {
        fn drop(&mut self) {
            match self.original.take() {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
        }
    }

    fn seed_claude_session(project: &Path, session_id: &str, body: &str) -> PathBuf {
        let hash = claude_project_hash(project.to_str().unwrap());
        let dir = dirs::home_dir()
            .unwrap()
            .join(".claude")
            .join("projects")
            .join(&hash);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{session_id}.jsonl"));
        fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn archive_move_and_restore_round_trip() {
        let _home_lock = HOME_LOCK.lock();
        let _db = ARCHIVE_DB_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = HomeGuard::new("roundtrip");
        let project = home.path().join("proj");
        fs::create_dir_all(&project).unwrap();
        let project_s = project.to_str().unwrap();
        let sid = format!("ua-rt-{}", uuid::Uuid::new_v4());
        let live = seed_claude_session(&project, &sid, "{\"type\":\"user\"}\n");
        assert!(live.exists(), "seed live file");
        assert!(resolve_claude_session_file(&sid, project_s).is_some());

        archive_user_session(project_s, "claude", &sid, "Roundtrip Chat", 1_700_000_000_000)
            .expect("archive");

        assert!(!live.exists(), "live file must be MOVEd away");
        let (arch, _) = user_archive_claude_paths(project_s, &sid);
        assert!(arch.exists(), "archive dest must exist: {}", arch.display());
        assert!(is_archived("claude", &sid), "DB archived flag");

        restore_user_session(project_s, "claude", &sid).expect("restore");
        assert!(!arch.exists(), "archive file gone after restore");
        assert!(
            resolve_claude_session_file(&sid, project_s).is_some(),
            "live file restored"
        );
        assert!(!is_archived("claude", &sid), "DB cleared");
    }

    #[test]
    fn soft_archive_when_live_file_missing() {
        let _home_lock = HOME_LOCK.lock();
        let _db = ARCHIVE_DB_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = HomeGuard::new("soft");
        let project = home.path().join("proj");
        fs::create_dir_all(&project).unwrap();
        let project_s = project.to_str().unwrap();
        let sid = format!("ua-soft-{}", uuid::Uuid::new_v4());

        archive_user_session(project_s, "claude", &sid, "Ghost", 1_700_000_000_001)
            .expect("soft archive");
        assert!(is_archived("claude", &sid));
        let (arch, _) = user_archive_claude_paths(project_s, &sid);
        assert!(!arch.exists(), "soft archive does not create file");

        restore_user_session(project_s, "claude", &sid).expect("restore soft");
        assert!(!is_archived("claude", &sid));
    }

    #[test]
    fn list_synthesizes_archived_after_move() {
        let _home_lock = HOME_LOCK.lock();
        let _db = ARCHIVE_DB_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = HomeGuard::new("synth");
        let project = home.path().join("proj");
        fs::create_dir_all(&project).unwrap();
        let project_s = project.to_str().unwrap();
        let sid = format!("ua-synth-{}", uuid::Uuid::new_v4());
        seed_claude_session(&project, &sid, "{\"type\":\"user\"}\n");

        archive_user_session(project_s, "claude", &sid, "Synth Title", 1_700_000_000_002)
            .expect("archive");

        // Live parse no longer sees the file; dual-query list must synthesize.
        let sessions = list_all_sessions(Some(project_s)).expect("list");
        let found = sessions
            .iter()
            .find(|s| s.session_id == sid && s.provider == "claude")
            .unwrap_or_else(|| panic!("archived session missing from list: {sessions:?}"));
        assert!(found.archived, "must be marked archived");
        assert_eq!(found.title, "Synth Title");
        assert_eq!(found.timestamp, 1_700_000_000_002);
    }

    #[test]
    fn dual_query_keeps_archived_when_active_overflows_cap() {
        let _home_lock = HOME_LOCK.lock();
        let _db = ARCHIVE_DB_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = HomeGuard::new("dual");
        let project = home.path().join("proj");
        fs::create_dir_all(&project).unwrap();
        let project_s = project.to_str().unwrap();

        // Archive one session first so it has a known id.
        let archived_sid = format!("ua-dual-arch-{}", uuid::Uuid::new_v4());
        seed_claude_session(&project, &archived_sid, "archived\n");
        archive_user_session(
            project_s,
            "claude",
            &archived_sid,
            "Must Survive Cap",
            1_000, // old timestamp
        )
        .expect("archive");

        // Seed 110 live sessions with newer timestamps so top-100 would
        // drop the archived one if dual-query failed.
        let hash = claude_project_hash(project_s);
        let dir = dirs::home_dir()
            .unwrap()
            .join(".claude")
            .join("projects")
            .join(&hash);
        for i in 0..110 {
            let sid = format!("ua-dual-live-{i:03}-{}", uuid::Uuid::new_v4());
            let path = dir.join(format!("{sid}.jsonl"));
            fs::write(&path, format!("live-{i}\n")).unwrap();
            // Bump mtime slightly by rewriting — order doesn't matter as
            // long as they're all present; archived is not on disk.
            let _ = path;
        }

        let sessions = list_all_sessions(Some(project_s)).expect("list");
        let active: Vec<_> = sessions.iter().filter(|s| !s.archived).collect();
        let archived: Vec<_> = sessions.iter().filter(|s| s.archived).collect();
        assert!(
            active.len() <= 100,
            "active cap 100, got {}",
            active.len()
        );
        assert!(
            archived.iter().any(|s| s.session_id == archived_sid),
            "archived session must survive top-100 active truncate; got {sessions:?}"
        );
    }

    #[test]
    fn rejects_non_claude_provider() {
        let _db = ARCHIVE_DB_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let err = archive_user_session("/tmp/p", "cursor", "x", "t", 0).unwrap_err();
        assert!(err.contains("claude"), "got: {err}");
    }
}
