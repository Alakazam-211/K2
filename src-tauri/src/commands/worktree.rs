//! Phase 2.1 wrap-up — generic worktree filesystem readers.
//!
//! Thin Tauri command(s) that expose read-only access to files inside
//! a known-good worktree path (the path the renderer already has via
//! `workspaces.worktree_path`). The primary consumer is the worktree
//! detail pane's "Task" tab, which renders `<worktree>/CLAUDE.md` as
//! Markdown.
//!
//! ## Why a Tauri command, not a daemon route?
//!
//! `feedback_daemon_first.md` says logic belongs in the daemon. This
//! is an explicit judgment-call exception: the worktree path is a
//! local filesystem path the renderer already received via the
//! existing `workspaces` row sync. There's no daemon-state involved
//! in a filesystem read — bouncing through the daemon's HTTP server
//! would add a hop for zero value. The relative-path validation is
//! pure (no DB, no IPC) so colocating it with the Tauri shim is fine.
//!
//! ## Safety
//!
//! `relative_path` is rejected if it:
//! - is empty
//! - is absolute (starts with `/` or contains a drive prefix)
//! - contains any `..` segment after normalization
//! - resolves outside `worktree_path` after canonicalization
//!
//! The canonicalization step is the load-bearing check — defeats
//! symlink-escape attacks the textual `..` filter would miss.

use std::path::{Component, Path, PathBuf};

/// Read a file at `relative_path` inside `worktree_path` and return
/// its UTF-8 contents.
///
/// Returns `Err("not_found")` if the file doesn't exist, and any other
/// short error string for unsafe paths / IO failures. The error strings
/// are stable enough for the renderer to switch on (e.g. show a
/// friendly empty state for `not_found`).
#[tauri::command]
pub fn read_worktree_file(
    worktree_path: String,
    relative_path: String,
) -> Result<String, String> {
    let content = read_worktree_file_inner(&worktree_path, &relative_path)?;
    Ok(content)
}

// ── Internal (testable without Tauri) ────────────────────────────────

fn read_worktree_file_inner(worktree_path: &str, relative_path: &str) -> Result<String, String> {
    let safe = resolve_safe_path(worktree_path, relative_path)?;
    match std::fs::read_to_string(&safe) {
        Ok(s) => Ok(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err("not_found".to_string()),
        Err(e) => Err(format!("read_failed: {}", e)),
    }
}

/// Path-validation + canonicalization. Returns the absolute, real
/// filesystem path on success.
fn resolve_safe_path(worktree_path: &str, relative_path: &str) -> Result<PathBuf, String> {
    if relative_path.is_empty() {
        return Err("empty_relative_path".to_string());
    }

    let rel = Path::new(relative_path);
    if rel.is_absolute() {
        return Err("absolute_relative_path".to_string());
    }
    // Reject `..` segments before joining — defeats trivial traversal.
    for component in rel.components() {
        match component {
            Component::ParentDir => return Err("parent_dir_segment".to_string()),
            Component::Prefix(_) | Component::RootDir => {
                return Err("absolute_relative_path".to_string())
            }
            _ => {}
        }
    }

    let worktree = PathBuf::from(worktree_path);
    if !worktree.is_absolute() {
        return Err("worktree_path_not_absolute".to_string());
    }

    // Canonicalize the worktree root once. We need the real (symlink-
    // resolved) path so an `eq` check against the resolved file path
    // is meaningful.
    let canonical_worktree = std::fs::canonicalize(&worktree)
        .map_err(|e| format!("worktree_not_found: {}", e))?;

    let joined = worktree.join(rel);

    // If the file exists, canonicalize the full path and verify it's
    // still under the worktree root. If it doesn't exist, we still
    // need to defend against parent symlinks that could redirect a
    // not-yet-created path outside the root — canonicalize the
    // parent directory instead.
    let canonical_file = match std::fs::canonicalize(&joined) {
        Ok(p) => p,
        Err(_) => {
            // File missing — canonicalize the parent and re-join the
            // leaf. The leaf can't redirect (it's just a name).
            let parent = joined
                .parent()
                .ok_or_else(|| "no_parent".to_string())?;
            let leaf = joined
                .file_name()
                .ok_or_else(|| "no_file_name".to_string())?;
            let canonical_parent = std::fs::canonicalize(parent)
                .map_err(|e| format!("parent_not_found: {}", e))?;
            canonical_parent.join(leaf)
        }
    };

    if !canonical_file.starts_with(&canonical_worktree) {
        return Err("path_escapes_worktree".to_string());
    }

    Ok(canonical_file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_worktree() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("CLAUDE.md"), "# Task brief\n\nDo the thing.\n")
            .expect("write CLAUDE.md");
        fs::create_dir_all(dir.path().join("sub")).expect("mkdir sub");
        fs::write(dir.path().join("sub/note.txt"), "hello\n").expect("write nested");
        dir
    }

    #[test]
    fn reads_claude_md_at_root() {
        let wt = temp_worktree();
        let content =
            read_worktree_file_inner(&wt.path().to_string_lossy(), "CLAUDE.md").expect("ok");
        assert!(content.contains("Task brief"));
    }

    #[test]
    fn reads_nested_file() {
        let wt = temp_worktree();
        let content =
            read_worktree_file_inner(&wt.path().to_string_lossy(), "sub/note.txt").expect("ok");
        assert_eq!(content, "hello\n");
    }

    #[test]
    fn missing_file_returns_not_found() {
        let wt = temp_worktree();
        let err = read_worktree_file_inner(&wt.path().to_string_lossy(), "MISSING.md")
            .expect_err("should fail");
        assert_eq!(err, "not_found");
    }

    #[test]
    fn rejects_empty_relative_path() {
        let wt = temp_worktree();
        let err = read_worktree_file_inner(&wt.path().to_string_lossy(), "")
            .expect_err("should fail");
        assert_eq!(err, "empty_relative_path");
    }

    #[test]
    fn rejects_absolute_relative_path() {
        let wt = temp_worktree();
        let err = read_worktree_file_inner(&wt.path().to_string_lossy(), "/etc/passwd")
            .expect_err("should fail");
        assert_eq!(err, "absolute_relative_path");
    }

    #[test]
    fn rejects_dotdot_segment() {
        let wt = temp_worktree();
        let err = read_worktree_file_inner(&wt.path().to_string_lossy(), "../../etc/passwd")
            .expect_err("should fail");
        assert_eq!(err, "parent_dir_segment");
    }

    #[test]
    fn rejects_dotdot_in_middle_of_path() {
        let wt = temp_worktree();
        let err = read_worktree_file_inner(&wt.path().to_string_lossy(), "sub/../../etc/passwd")
            .expect_err("should fail");
        assert_eq!(err, "parent_dir_segment");
    }

    #[test]
    fn rejects_symlink_escape() {
        // Create a symlink inside the worktree that points outside,
        // then attempt to read a file via that symlink. The textual
        // `..` filter doesn't see it; canonicalization does.
        let outside = tempfile::tempdir().expect("outside tempdir");
        fs::write(outside.path().join("secret.txt"), "leaked\n").expect("write secret");

        let wt = temp_worktree();
        let link_path = wt.path().join("escape");
        #[cfg(unix)]
        std::os::unix::fs::symlink(outside.path(), &link_path).expect("symlink");
        // On non-unix this test is a no-op (skipped); the symlink crate
        // call would need different handling.

        #[cfg(unix)]
        {
            let err =
                read_worktree_file_inner(&wt.path().to_string_lossy(), "escape/secret.txt")
                    .expect_err("should reject");
            assert_eq!(err, "path_escapes_worktree", "symlink escape was not rejected");
        }
    }

    #[test]
    fn rejects_non_absolute_worktree_path() {
        let err =
            read_worktree_file_inner("relative/path", "CLAUDE.md").expect_err("should fail");
        assert_eq!(err, "worktree_path_not_absolute");
    }
}
