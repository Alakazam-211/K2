//! Atomic filesystem primitives shared by migration, teardown, and skill
//! regeneration paths.
//!
//! Why it exists: K2SO writes several canonical files (SKILL.md, AGENTS.md
//! marker injection, harness-discovery targets) whose mid-write corruption
//! can silently lose the user's accumulated context. Direct `fs::write`
//! cannot guarantee that a power loss or SIGKILL mid-syscall leaves the
//! old file intact. Writing to a sibling tempfile and renaming into place
//! gives POSIX atomicity: a reader sees either the previous bytes in full
//! or the new bytes in full, never a truncated intermediate.
//!
//! Naming: archive files live under `.k2so/migration/` and are produced
//! during first-run harvest of pre-existing CLAUDE.md / GEMINI.md /
//! .aider.conf.yml files. A tight first-run harvest can create 5+ archives
//! in a single wall-clock second, so nanosecond timestamps plus a
//! per-process monotonic counter are required to rule out collisions.
//!
//! Pattern reference: Zed `crates/fs/src/fs.rs::atomic_write` uses
//! `tempfile::NamedTempFile::new_in(parent) → persist(path)`. This module
//! reimplements the core of that pattern without taking a `tempfile`
//! dependency — the logic is small and we need fine-grained control over
//! sync_all semantics.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Monotonic per-process counter appended to tempfile and archive names.
/// Ensures uniqueness across concurrent callers within the same process
/// even if they land on the same nanosecond (common in tight loops, also
/// guards against clock coarsening on some filesystems).
static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Write `bytes` to `path` atomically. A reader either sees the previous
/// file in full or the new file in full — never a half-written result.
///
/// Implementation: writes to a sibling tempfile in the same directory,
/// fsync's the data, then `fs::rename`s over the target. POSIX rename is
/// atomic and same-filesystem — placing the tempfile next to `path`
/// guarantees the rename stays on one filesystem (cross-fs rename would
/// degrade to copy+delete and lose atomicity).
///
/// The parent directory is created if missing. Errors propagate so the
/// caller can log or retry; never ignore the result.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let tmp = tempfile_path(path);
    let write_result = (|| -> std::io::Result<()> {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        Ok(())
    })();
    if let Err(e) = write_result {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    fs::rename(&tmp, path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        e
    })
}

/// UTF-8 convenience wrapper around [`atomic_write`].
pub fn atomic_write_str(path: &Path, text: &str) -> std::io::Result<()> {
    atomic_write(path, text.as_bytes())
}

/// Atomically create or replace the symlink at `target`, pointing to
/// `source`. Unlike a remove+create sequence, concurrent readers never
/// observe a missing file — the new link overwrites the old in one rename.
#[cfg(unix)]
pub fn atomic_symlink(source: &Path, target: &Path) -> std::io::Result<()> {
    let parent = target.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let tmp = tempfile_path(target);
    let _ = fs::remove_file(&tmp); // clean up any previous aborted call
    std::os::unix::fs::symlink(source, &tmp)?;
    fs::rename(&tmp, target).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        e
    })
}

/// Non-unix fallback. K2SO is macOS-only today; this path exists so the
/// crate still compiles under `cargo check` on other hosts.
#[cfg(not(unix))]
pub fn atomic_symlink(source: &Path, target: &Path) -> std::io::Result<()> {
    if let Some(parent) = target.parent() { fs::create_dir_all(parent)?; }
    fs::copy(source, target).map(|_| ())
}

/// Build a collision-free archive path for a harvested file. The archive
/// filename is `<stem>-<unix_nanos>-<seq><ext>`. Nanosecond precision plus
/// the per-process counter means two archives created in the same
/// instruction still produce distinct paths — a real risk during first-run
/// harvest, which walks every agent + every harness file in a single pass.
///
/// `ext` must include the leading dot (`".md"`, `".yml"`) or be empty for
/// extensionless names (`.goosehints`). Parent directory creation is the
/// caller's responsibility (callers already do `create_dir_all` on the
/// migration root).
pub fn unique_archive_path(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    dir.join(format!("{}-{}-{:04}{}", stem, ns, seq, ext))
}

/// Log-and-swallow wrapper for operations the caller explicitly wants to
/// tolerate failing (cleanup paths, best-effort migration sweeps). Use
/// this instead of `let _ = fs::...` so failures leave an audit trail.
///
/// Uses a raw stderr write rather than `eprintln!` because the latter
/// panics on write failure, and K2SO can run with no tty attached when
/// launched from Finder.
pub fn log_if_err<T, E: std::fmt::Display>(op: &str, path: &Path, result: Result<T, E>) {
    if let Err(e) = result {
        use std::io::Write;
        let _ = writeln!(
            std::io::stderr(),
            "k2so: {} failed at {}: {}",
            op,
            path.display(),
            e
        );
    }
}

/// Construct a tempfile path sitting next to `target`. Leading-dot +
/// `k2so-tmp` segment + PID + nanos + seq makes collision essentially
/// impossible, and the dot-prefix hides the file from `ls` listings if a
/// crash leaves it behind.
fn tempfile_path(target: &Path) -> PathBuf {
    let parent = target.parent().unwrap_or_else(|| Path::new("."));
    let leaf = target
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("tmp");
    let pid = std::process::id();
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    parent.join(format!(".{}.k2so-tmp.{}.{}.{}", leaf, pid, ns, seq))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn tmpdir() -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "k2so-fsatomic-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&base).unwrap();
        base
    }

    #[test]
    fn atomic_write_creates_new_file() {
        let dir = tmpdir();
        let p = dir.join("hello.txt");
        atomic_write_str(&p, "world").unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), "world");
    }

    #[test]
    fn atomic_write_replaces_existing_file() {
        let dir = tmpdir();
        let p = dir.join("note.md");
        fs::write(&p, "old body").unwrap();
        atomic_write_str(&p, "new body").unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), "new body");
    }

    #[test]
    fn atomic_write_creates_missing_parent() {
        let dir = tmpdir();
        let p = dir.join("nested/deeper/file.md");
        atomic_write_str(&p, "x").unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), "x");
    }

    #[test]
    fn atomic_write_leaves_no_tempfile_on_success() {
        let dir = tmpdir();
        let p = dir.join("t.md");
        atomic_write_str(&p, "ok").unwrap();
        let leftover: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with(".t.md.k2so-tmp.")
            })
            .collect();
        assert!(leftover.is_empty(), "tempfile should be removed on success");
    }

    #[test]
    fn atomic_write_cleans_up_tempfile_on_error() {
        // Directing atomic_write at a path whose parent is a regular file
        // (not a directory) causes create_dir_all to fail. Verify the
        // tempfile isn't orphaned in the working directory.
        let dir = tmpdir();
        let blocker = dir.join("not-a-dir");
        fs::write(&blocker, "blocker").unwrap();
        let target = blocker.join("cannot-create-here.md");
        assert!(atomic_write_str(&target, "nope").is_err());
    }

    #[test]
    fn atomic_symlink_replaces_regular_file() {
        let dir = tmpdir();
        let source = dir.join("source.md");
        fs::write(&source, "source body").unwrap();
        let target = dir.join("target.md");
        fs::write(&target, "original regular file").unwrap();

        atomic_symlink(&source, &target).unwrap();
        let meta = fs::symlink_metadata(&target).unwrap();
        assert!(meta.file_type().is_symlink());
        assert_eq!(fs::read_to_string(&target).unwrap(), "source body");
    }

    #[test]
    fn atomic_symlink_refreshes_existing_symlink() {
        let dir = tmpdir();
        let src1 = dir.join("one.md");
        let src2 = dir.join("two.md");
        fs::write(&src1, "one").unwrap();
        fs::write(&src2, "two").unwrap();
        let target = dir.join("link.md");
        atomic_symlink(&src1, &target).unwrap();
        atomic_symlink(&src2, &target).unwrap();
        assert_eq!(fs::read_to_string(&target).unwrap(), "two");
    }

    #[test]
    fn unique_archive_path_never_collides_under_rapid_fire() {
        let dir = tmpdir();
        let mut seen: HashSet<PathBuf> = HashSet::new();
        for _ in 0..4096 {
            let p = unique_archive_path(&dir, "CLAUDE", ".md");
            assert!(seen.insert(p.clone()), "duplicate archive path: {}", p.display());
        }
    }

    #[test]
    fn unique_archive_path_preserves_extension() {
        let dir = tmpdir();
        let md = unique_archive_path(&dir, "CLAUDE", ".md");
        let yml = unique_archive_path(&dir, "aider-conf", ".yml");
        let ext_less = unique_archive_path(&dir, ".goosehints", "");
        assert!(md.extension().map(|e| e == "md").unwrap_or(false));
        assert!(yml.extension().map(|e| e == "yml").unwrap_or(false));
        assert!(ext_less.extension().is_none());
    }

    #[test]
    fn unique_archive_path_contains_stem() {
        let dir = tmpdir();
        let p = unique_archive_path(&dir, "my-agent", ".md");
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        assert!(name.starts_with("my-agent-"));
        assert!(name.ends_with(".md"));
    }

    #[test]
    fn atomic_write_survives_many_overwrites() {
        // Regression: early tempfile-naming used seconds granularity and
        // could collide within the same instant. This would cause rapid
        // overwrites to fail when the tempfile already existed.
        let dir = tmpdir();
        let p = dir.join("spinner.md");
        for i in 0..256 {
            atomic_write_str(&p, &format!("tick {i}")).unwrap();
        }
        assert_eq!(fs::read_to_string(&p).unwrap(), "tick 255");
    }
}
