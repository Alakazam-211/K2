//! Session archive — protect agent transcripts from provider reaping.
//!
//! Claude Code (and other providers) delete local session transcripts
//! after a retention window (~30 days by default). K2's SQLite rows
//! (`workspace_sessions.session_id` etc.) outlive them, leaving sessions
//! that can never be resumed — the file behind the id is simply gone.
//! Verified on Rosson's machine 2026-07-08: 10 of 30 DB-tracked sessions
//! had no `.jsonl` anywhere.
//!
//! Defense: a daily sweep COPIES (never moves — moving breaks
//! `--resume`) any session file older than `session_archive_days` into:
//!   - `<project>/.k2/session-archive/claude/<slug>/…` for sessions
//!     belonging to a known K2 project (rides clones + migrations with
//!     the workspace, by design), and
//!   - `~/.k2/session-archive/claude/<slug>/…` as a global catch-all
//!     for every other slug on the machine (projects K2 doesn't manage).
//!
//! Copies are incremental: a file is re-copied only when the source is
//! newer than the archived copy. `memory/` subtrees are skipped (not a
//! reap target). Restore (copying an archive back so a session resumes
//! again) and DB reference-map reconciliation are planned follow-ups —
//! see Rosson 2026-07-08.
//!
//! Naming: `session-archive` (dash) per house convention
//! (connect-hosts.json, heartbeat-projects.txt, claude-auth-refresh.*).

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::chat_history::claude_project_hash;

/// Result of one sweep (per project or global).
#[derive(Debug, Default, Clone, Copy)]
pub struct ArchiveStats {
    pub copied: u64,
    pub skipped: u64,
    pub bytes_copied: u64,
    pub errors: u64,
}

impl ArchiveStats {
    fn absorb(&mut self, other: ArchiveStats) {
        self.copied += other.copied;
        self.skipped += other.skipped;
        self.bytes_copied += other.bytes_copied;
        self.errors += other.errors;
    }
}

/// Is this file part of a session transcript we protect?
/// `.jsonl` = the transcript itself (including `subagents/*.jsonl`);
/// `.meta.json` = subagent metadata that keeps transcripts interpretable.
fn is_session_file(name: &str) -> bool {
    name.ends_with(".jsonl") || name.ends_with(".meta.json")
}

/// Age gate, pure for testability. `min_age_days == 0` disables the
/// sweep entirely (never eligible) — "archive everything immediately"
/// is expressed as `1`, not `0`, so a zeroed setting is a clean OFF.
pub fn eligible(age: Duration, min_age_days: u32) -> bool {
    if min_age_days == 0 {
        return false;
    }
    age >= Duration::from_secs(u64::from(min_age_days) * 86_400)
}

/// Should we (re)copy `src` over the archived copy? Pure: copy when no
/// archive exists, the source is newer than the archive, or the sizes
/// differ.
///
/// The size check matters: Linux stamps mtime from the coarse kernel
/// clock, so a transcript that grows within the same tick as the last
/// sweep keeps the archive's (copied) mtime. Comparing mtime alone would
/// never re-archive that growth until the file is written again.
pub fn needs_copy(src_mtime: SystemTime, src_len: u64, dest: Option<(SystemTime, u64)>) -> bool {
    match dest {
        None => true,
        Some((dest_mtime, dest_len)) => src_mtime > dest_mtime || src_len != dest_len,
    }
}

fn file_mtime_len(p: &Path) -> Option<(SystemTime, u64)> {
    let meta = std::fs::metadata(p).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

/// Copy every eligible session file under `src_slug_dir` into
/// `dest_slug_dir`, preserving relative layout. Skips `memory/`.
fn archive_slug_dir(
    src_slug_dir: &Path,
    dest_slug_dir: &Path,
    min_age_days: u32,
    now: SystemTime,
) -> ArchiveStats {
    let mut stats = ArchiveStats::default();
    let mut pending: Vec<PathBuf> = vec![src_slug_dir.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => {
                stats.errors += 1;
                continue;
            }
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if path.is_dir() {
                if name == "memory" {
                    continue; // not a reap target; workspace memory has its own life
                }
                pending.push(path);
                continue;
            }
            if !is_session_file(&name) {
                continue;
            }
            let Some((mtime, len)) = file_mtime_len(&path) else {
                stats.errors += 1;
                continue;
            };
            let age = now.duration_since(mtime).unwrap_or(Duration::ZERO);
            if !eligible(age, min_age_days) {
                continue;
            }
            let rel = match path.strip_prefix(src_slug_dir) {
                Ok(r) => r,
                Err(_) => {
                    stats.errors += 1;
                    continue;
                }
            };
            let dest = dest_slug_dir.join(rel);
            if !needs_copy(mtime, len, file_mtime_len(&dest)) {
                stats.skipped += 1;
                continue;
            }
            if let Some(parent) = dest.parent() {
                if std::fs::create_dir_all(parent).is_err() {
                    stats.errors += 1;
                    continue;
                }
            }
            match std::fs::copy(&path, &dest) {
                Ok(bytes) => {
                    // Preserve the source mtime so the "source newer?"
                    // incremental check stays meaningful across sweeps.
                    if let Ok(meta) = std::fs::metadata(&path) {
                        if let Ok(m) = meta.modified() {
                            let _ = filetime_set(&dest, m);
                        }
                    }
                    stats.copied += 1;
                    stats.bytes_copied += bytes;
                }
                Err(_) => stats.errors += 1,
            }
        }
    }
    stats
}

/// Set a file's mtime without adding a crate dependency: open+set via
/// std is not available on stable, so fall back to a best-effort touch
/// through `File::set_modified` (stable since 1.75).
fn filetime_set(path: &Path, mtime: SystemTime) -> std::io::Result<()> {
    let f = std::fs::OpenOptions::new().write(true).open(path)?;
    f.set_modified(mtime)
}

/// Archive one K2 project's Claude sessions into the project's own
/// `.k2/session-archive/` (so archives ride clones + migrations).
pub fn archive_project_sessions(
    home: &Path,
    project_path: &str,
    min_age_days: u32,
    now: SystemTime,
) -> ArchiveStats {
    let slug = claude_project_hash(project_path);
    let src = home.join(".claude").join("projects").join(&slug);
    if !src.is_dir() {
        return ArchiveStats::default();
    }
    let dest = Path::new(project_path)
        .join(".k2")
        .join("session-archive")
        .join("claude")
        .join(&slug);
    archive_slug_dir(&src, &dest, min_age_days, now)
}

/// Archive every slug NOT claimed by a known project into the global
/// catch-all at `~/.k2/session-archive/claude/<slug>/`.
pub fn archive_global_catchall(
    home: &Path,
    known_slugs: &[String],
    min_age_days: u32,
    now: SystemTime,
) -> ArchiveStats {
    let projects_dir = home.join(".claude").join("projects");
    let dest_root = home.join(".k2").join("session-archive").join("claude");
    let mut stats = ArchiveStats::default();
    let Ok(entries) = std::fs::read_dir(&projects_dir) else {
        return stats;
    };
    for entry in entries.flatten() {
        let slug_dir = entry.path();
        if !slug_dir.is_dir() {
            continue;
        }
        let slug = entry.file_name().to_string_lossy().into_owned();
        if known_slugs.iter().any(|k| k == &slug) {
            continue; // its project archive owns it
        }
        stats.absorb(archive_slug_dir(
            &slug_dir,
            &dest_root.join(&slug),
            min_age_days,
            now,
        ));
    }
    stats
}

/// One full daily sweep: every known project into its workspace archive,
/// everything else into the global catch-all. Reads the day threshold
/// from app settings (`session_archive_days`, 0 = disabled).
pub fn run_daily_sweep() -> ArchiveStats {
    let min_age_days = crate::app_settings::load().session_archive_days;
    let mut stats = ArchiveStats::default();
    if min_age_days == 0 {
        return stats;
    }
    let Some(home) = dirs::home_dir() else {
        return stats;
    };
    let now = SystemTime::now();

    // Known projects from the DB (path column). Best-effort: a DB error
    // degrades to catch-all-only, never a panic in the daemon tick.
    let project_paths: Vec<String> = {
        let db = crate::db::shared();
        let conn = db.lock();
        conn.prepare("SELECT path FROM projects WHERE path IS NOT NULL AND path != ''")
            .and_then(|mut st| {
                st.query_map([], |r| r.get::<_, String>(0))
                    .map(|rows| rows.flatten().collect())
            })
            .unwrap_or_default()
    };

    let mut known_slugs = Vec::with_capacity(project_paths.len());
    for path in &project_paths {
        known_slugs.push(claude_project_hash(path));
        stats.absorb(archive_project_sessions(&home, path, min_age_days, now));
    }
    stats.absorb(archive_global_catchall(
        &home,
        &known_slugs,
        min_age_days,
        now,
    ));
    crate::log_debug!(
        "[session-archive] daily sweep: copied={} skipped={} bytes={} errors={} (threshold {}d, {} projects)",
        stats.copied,
        stats.skipped,
        stats.bytes_copied,
        stats.errors,
        min_age_days,
        project_paths.len()
    );
    stats
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_days_means_disabled_not_archive_everything() {
        assert!(!eligible(Duration::from_secs(86_400 * 400), 0));
        assert!(eligible(Duration::from_secs(86_400), 1));
        assert!(!eligible(Duration::from_secs(3600), 1));
    }

    #[test]
    fn needs_copy_only_when_source_newer_resized_or_missing() {
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
        let t1 = SystemTime::UNIX_EPOCH + Duration::from_secs(2_000);
        assert!(needs_copy(t0, 5, None));
        assert!(needs_copy(t1, 5, Some((t0, 5))), "newer source");
        assert!(!needs_copy(t0, 5, Some((t0, 5))), "same mtime, same size");
        assert!(!needs_copy(t0, 5, Some((t1, 5))), "older source, same size");
        // Same-tick growth: mtime unchanged, size differs -> copy.
        assert!(needs_copy(t0, 9, Some((t0, 5))), "same mtime, grew");
        assert!(needs_copy(t0, 3, Some((t0, 5))), "same mtime, shrank");
    }

    #[test]
    fn session_file_filter() {
        assert!(is_session_file("abc.jsonl"));
        assert!(is_session_file("agent-x.meta.json"));
        assert!(!is_session_file("MEMORY.md"));
        assert!(!is_session_file("notes.txt"));
    }

    /// A fresh, unique scratch dir (uuid, not pid/nanos: those repeat
    /// across test binaries and runs).
    fn scratch_dir() -> PathBuf {
        std::env::temp_dir().join(format!("k2-session-archive-test-{}", uuid::Uuid::new_v4()))
    }

    #[test]
    fn archives_copy_incrementally_and_skip_memory() {
        let tmp = scratch_dir();
        let src = tmp.join("slug");
        let dest = tmp.join("archive");
        std::fs::create_dir_all(src.join("subagents")).unwrap();
        std::fs::create_dir_all(src.join("memory")).unwrap();
        std::fs::write(src.join("s1.jsonl"), b"one").unwrap();
        std::fs::write(src.join("subagents/a1.jsonl"), b"two").unwrap();
        std::fs::write(src.join("subagents/a1.meta.json"), b"m").unwrap();
        std::fs::write(src.join("memory/MEMORY.md"), b"keep out").unwrap();
        std::fs::write(src.join("readme.txt"), b"not a session").unwrap();
        let t0 = std::fs::metadata(src.join("s1.jsonl"))
            .unwrap()
            .modified()
            .unwrap();

        // min_age 1 day: fresh files aren't eligible.
        let s = archive_slug_dir(&src, &dest, 1, SystemTime::now());
        assert_eq!(s.copied, 0);

        // Pretend a day passed by evaluating "now" in the future.
        let later = SystemTime::now() + Duration::from_secs(2 * 86_400);
        let s = archive_slug_dir(&src, &dest, 1, later);
        assert_eq!(s.copied, 3, "two jsonl + one meta.json");
        assert!(dest.join("s1.jsonl").exists());
        assert!(dest.join("subagents/a1.jsonl").exists());
        assert!(!dest.join("memory").exists(), "memory/ must be skipped");
        assert!(!dest.join("readme.txt").exists());

        // Second sweep: all current -> skipped, nothing re-copied.
        let s = archive_slug_dir(&src, &dest, 1, later);
        assert_eq!(s.copied, 0);
        assert_eq!(s.skipped, 3);

        // Source grows with a clearly newer mtime -> re-copied. The mtime
        // is set explicitly: the kernel's coarse clock would otherwise
        // stamp the rewrite with the same tick as the first write.
        std::fs::write(src.join("s1.jsonl"), b"one-more-turn").unwrap();
        filetime_set(&src.join("s1.jsonl"), t0 + Duration::from_secs(10)).unwrap();
        let s = archive_slug_dir(&src, &dest, 1, later);
        assert_eq!(s.copied, 1);
        assert_eq!(
            std::fs::read(dest.join("s1.jsonl")).unwrap(),
            b"one-more-turn"
        );

        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn same_tick_growth_is_recopied() {
        let tmp = scratch_dir();
        let src = tmp.join("slug");
        let dest = tmp.join("archive");
        std::fs::create_dir_all(&src).unwrap();
        let file = src.join("s1.jsonl");
        std::fs::write(&file, b"one").unwrap();
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        filetime_set(&file, t0).unwrap();

        let later = SystemTime::now() + Duration::from_secs(2 * 86_400);
        let s = archive_slug_dir(&src, &dest, 1, later);
        assert_eq!(s.copied, 1);

        // Grow the transcript but pin the mtime to the archived one: the
        // same-tick case Linux produces for back-to-back writes.
        std::fs::write(&file, b"one-more-turn").unwrap();
        filetime_set(&file, t0).unwrap();
        let s = archive_slug_dir(&src, &dest, 1, later);
        assert_eq!(s.copied, 1, "size changed within the same mtime tick");
        assert_eq!(std::fs::read(dest.join("s1.jsonl")).unwrap(), b"one-more-turn");

        // And now it is current: nothing to do.
        let s = archive_slug_dir(&src, &dest, 1, later);
        assert_eq!(s.copied, 0);
        assert_eq!(s.skipped, 1);

        std::fs::remove_dir_all(&tmp).unwrap();
    }
}
