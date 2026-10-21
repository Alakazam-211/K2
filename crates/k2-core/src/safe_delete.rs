//! Safe deletion — sends paths to the OS recycle bin instead of
//! permanent unlink. **Never permanently deletes user-authored
//! content.**
//!
//! ## Why
//!
//! 0.37.5 shipped a user report: a workspace's user code got deleted
//! after a "move + pin" interaction. Forensics pointed at
//! `git.rs::remove_worktree`'s fallback path which calls
//! `git worktree remove` (permanent, no recycle bin) when the
//! rename-to-trash fast path fails. Per user directive: **every
//! deletion path that can touch user content goes to the recycle
//! bin. Permanent unlink is reserved for daemon scratch state we
//! own and can recreate (atomic-write temps, drained inbox files,
//! lock files, launchd plists, test cleanup).**
//!
//! ## What's a "user content" path?
//!
//! Anything where:
//! - The user manually created or edited content under it (their
//!   code, AGENT.md they tweaked, work-item markdown they wrote,
//!   skill layer files they authored).
//! - K2SO created it but losing it would surprise the user
//!   (CLAUDE.md, .aider.conf.yml, persona files).
//! - It's the workspace root, a worktree, or any subtree thereof.
//!
//! Helpers in this module take such paths and route them through
//! the OS recycle bin (`trash` crate — handles macOS, Windows,
//! freedesktop Linux). On platforms without a recycle bin (some
//! minimal Linux configs), `trash::delete` returns an error and
//! the helpers surface it; callers can choose to fall back to
//! permanent unlink with explicit acknowledgment, but the default
//! is "if we can't trash, we don't delete."
//!
//! ## What stays as `fs::remove_*`?
//!
//! - `fs_atomic.rs`: atomic-write temp files (the temp IS a
//!   half-finished write that needs cleanup; nothing to recover).
//! - `pending_live::drain*`: drained signal JSONs (already injected
//!   into a session; the file is consumed state, not user content).
//! - `awareness::inbox::drain`: same as above.
//! - `agents::session::*_unlock`: `.lock` file removal.
//! - Test cleanup paths: temp workspaces under `std::env::temp_dir()`.
//! - launchd plist install/uninstall: K2SO-owned, recreatable.
//!
//! Each remaining `fs::remove_*` call site has an inline comment
//! justifying why it's safe.

use std::path::Path;

/// Test builds only: the folder that stands in for the OS Trash
/// (`K2_TEST_TRASH_DIR`; `TempHome` sets it to `<home>/.Trash`).
pub const TEST_TRASH_DIR_ENV: &str = "K2_TEST_TRASH_DIR";

/// The ONE call into the OS recycle bin (the `trash` crate). Every helper
/// here and `fs_commands::delete` go through it.
///
/// 0.46.3: in test builds (`cfg(test)` or k2-core's `test-util`, which only
/// dev-dependencies enable) it never touches the real Trash: on macOS that
/// drives Finder over AppleEvents, which pops Touch ID on a desk Mac and
/// times out (-1712) on the headless compute minis. It moves the path into
/// [`TEST_TRASH_DIR_ENV`] instead, or `<temp>/k2-test-trash` when unset —
/// still a move, never an unlink, so "it went to the Trash" assertions keep
/// meaning something. A release build compiles none of this: production is
/// `trash::delete`, unchanged.
pub(crate) fn os_trash(path: &Path) -> Result<(), String> {
    #[cfg(any(test, feature = "test-util"))]
    {
        test_trash(path)
    }
    #[cfg(not(any(test, feature = "test-util")))]
    {
        trash::delete(path).map_err(|e| e.to_string())
    }
}

#[cfg(any(test, feature = "test-util"))]
fn test_trash(path: &Path) -> Result<(), String> {
    let dir = std::env::var_os(TEST_TRASH_DIR_ENV)
        .filter(|v| !v.is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("k2-test-trash"));
    std::fs::create_dir_all(&dir).map_err(|e| format!("test trash dir {}: {e}", dir.display()))?;
    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "item".into());
    let dest = dir.join(format!("{name}-{}", uuid::Uuid::new_v4().simple()));
    match std::fs::rename(path, &dest) {
        Ok(()) => Ok(()),
        // Another filesystem (a temp HOME on one volume, the item on
        // another): copy then remove, like a Trash move across volumes.
        Err(_) => {
            copy_recursive(path, &dest).map_err(|e| format!("test trash copy {}: {e}", path.display()))?;
            if path.is_dir() {
                std::fs::remove_dir_all(path)
            } else {
                std::fs::remove_file(path)
            }
            .map_err(|e| format!("test trash remove {}: {e}", path.display()))
        }
    }
}

#[cfg(any(test, feature = "test-util"))]
fn copy_recursive(from: &Path, to: &Path) -> std::io::Result<()> {
    let meta = std::fs::symlink_metadata(from)?;
    if meta.is_dir() {
        std::fs::create_dir_all(to)?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            copy_recursive(&entry.path(), &to.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        std::fs::copy(from, to).map(|_| ())
    }
}

/// Send a path (file or directory) to the OS recycle bin. Returns
/// an error if the trash operation fails (most commonly: missing
/// trash service on minimal Linux, permission denied, path doesn't
/// exist). Callers should surface the error rather than silently
/// fall back to permanent deletion — the whole point of this
/// helper is "don't permanently destroy user data."
pub fn trash<P: AsRef<Path>>(path: P) -> Result<(), String> {
    let path_ref = path.as_ref();
    if !path_ref.exists() {
        // Mirror `fs::remove_*` semantics: not-existing is success.
        return Ok(());
    }
    os_trash(path_ref).map_err(|e| {
        format!(
            "trash {} failed: {} (path NOT permanently deleted; intervene manually)",
            path_ref.display(),
            e
        )
    })
}

/// Best-effort trash. Swallows errors with a `log_debug!` line.
/// Use when the caller previously did `let _ = fs::remove_*(...)`
/// — keeps the same fire-and-forget semantics but routes through
/// the recycle bin. Returns `true` if the path was trashed (or
/// didn't exist), `false` if the trash op failed.
pub fn try_trash<P: AsRef<Path>>(path: P) -> bool {
    match trash(&path) {
        Ok(()) => true,
        Err(e) => {
            crate::log_debug!("[safe_delete] {e}");
            false
        }
    }
}

/// "Trash-preferred, fallback to permanent on failure" — for paths
/// where the content was K2SO-scaffolded with no user-authored
/// material (a SKILL.md file we wrote, a launchd plist we
/// installed). The directive "never permanent-delete user content"
/// still holds; this helper is for content where the worst-case
/// outcome is "K2SO has to re-scaffold a file it created."
///
/// Behavior:
/// 1. Try to trash. If success, return Ok.
/// 2. If trash fails (e.g. CI without Finder, AppleScript timeout,
///    Linux without freedesktop.org Trash service), log + fall
///    back to `fs::remove_*`. The path is gone either way.
///
/// **DO NOT use this for user-authored content** — use plain
/// `trash()` and let the error bubble. If trash fails on user
/// content, the safe answer is "leave it in place and let the
/// user investigate," not "delete it anyway."
pub fn trash_or_remove<P: AsRef<Path>>(path: P) -> std::io::Result<()> {
    let path_ref = path.as_ref();
    if !path_ref.exists() {
        return Ok(());
    }
    match os_trash(path_ref) {
        Ok(()) => Ok(()),
        Err(e) => {
            crate::log_debug!(
                "[safe_delete] trash {} failed ({e}); falling back to permanent delete (k2so-scratch path)",
                path_ref.display()
            );
            if path_ref.is_dir() {
                std::fs::remove_dir_all(path_ref)
            } else {
                std::fs::remove_file(path_ref)
            }
        }
    }
}

#[cfg(test)]
mod test_trash_tests {
    use super::*;

    /// Tests never reach the real Trash: the path moves into the test
    /// trash folder, content intact.
    #[test]
    fn trash_in_tests_moves_into_the_test_trash_dir() {
        let home = crate::test_env::TempHome::new();
        let trash_dir = std::path::PathBuf::from(
            std::env::var_os(TEST_TRASH_DIR_ENV).expect("TempHome sets the test trash dir"),
        );
        assert_eq!(trash_dir, home.path().join(".Trash"));
        let f = home.path().join("note.md");
        std::fs::write(&f, "keep me").unwrap();
        trash(&f).expect("trash");
        assert!(!f.exists(), "the file left its folder");
        let moved: Vec<_> = std::fs::read_dir(&trash_dir).unwrap().map(|e| e.unwrap().path()).collect();
        assert_eq!(moved.len(), 1, "{moved:?}");
        assert_eq!(std::fs::read_to_string(&moved[0]).unwrap(), "keep me", "moved, never unlinked");

        let d = home.path().join("folder");
        std::fs::create_dir_all(d.join("sub")).unwrap();
        std::fs::write(d.join("sub/x"), "x").unwrap();
        trash_or_remove(&d).expect("trash_or_remove");
        assert!(!d.exists());
        assert_eq!(std::fs::read_dir(&trash_dir).unwrap().count(), 2);
    }
}
