//! Test-aware trash helper — used by production call sites that are
//! reached by tests on temp-dir scratch paths.
//!
//! ## Why a separate module?
//!
//! [`crate::safe_delete::trash`] is the canonical "send to recycle bin"
//! chokepoint and is intentionally path-policy-blind: every caller
//! gets the same behavior, whether the path is a user's worktree or a
//! test scratch dir. That keeps the safety boundary auditable
//! (`feedback_recycle_bin_tests`: never permanent-delete user
//! content).
//!
//! The trade-off shows up in test contexts. On macOS the `trash` crate
//! shells through AppleEvents → Finder, which requires a TCC
//! permission grant. The K2SO.app bundle has that grant cached.
//! `cargo test` binaries and ad-hoc sandbox daemons spawned by
//! `tests/cli/*.sh` do NOT — every trash call prompts the user for
//! Touch ID, hangs the test run, and silently false-positives
//! "trashed=true" assertions when the AppleEvents call fails after
//! the prompt.
//!
//! [`safe_delete`]'s docs already list "Test cleanup paths: temp
//! workspaces under `std::env::temp_dir()`" as legitimate
//! `fs::remove_*` candidates. This helper exposes that policy as a
//! single function call: route through the trash crate for paths
//! outside `temp_dir()`, but skip straight to `fs::remove_*` for
//! paths under it. Production never trashes K2SO-scratch paths, so
//! the only behavioral change is "tests stop prompting Touch ID."
//!
//! ## When to use
//!
//! Wrap an existing `safe_delete::trash(p)` call with
//! `scratch_safe_trash(p)` when:
//!
//! - The call is in production code that is exercised by tests via
//!   the normal entrypoint (not a test-only fork).
//! - The path under deletion is K2SO-managed scaffolding (`.k2so/`
//!   subtrees in a workspace, agent dirs, migration roots) — never
//!   the workspace root itself or arbitrary user files.
//!
//! Plain `safe_delete::trash` is still the right call for user
//! content (worktree dirs, user-authored CLAUDE.md/AGENT.md, etc.).

use std::fs;
use std::path::Path;

/// Trash-preferred deletion that bypasses the OS recycle bin for
/// paths under `std::env::temp_dir()`. See module docs for the
/// rationale.
///
/// Behavior:
/// - Path doesn't exist: `Ok(())` (mirrors `safe_delete::trash`).
/// - Path under `std::env::temp_dir()`: permanent delete via
///   `fs::remove_dir_all` / `fs::remove_file`. No Touch ID prompt.
/// - Anywhere else: route through [`crate::safe_delete::trash`] as
///   normal (production path; user content goes to recycle bin).
pub fn scratch_safe_trash<P: AsRef<Path>>(path: P) -> Result<(), String> {
    let path_ref = path.as_ref();
    if !path_ref.exists() {
        return Ok(());
    }
    if path_is_under_temp_dir(path_ref) {
        // SAFETY: test scratch path under temp_dir() — bypass trash
        // crate to avoid TCC prompt. Permanent delete is acceptable
        // because the path is K2SO-owned scratch.
        let result = if path_ref.is_dir() {
            fs::remove_dir_all(path_ref)
        } else {
            fs::remove_file(path_ref)
        };
        return result.map_err(|e| {
            format!(
                "scratch remove {} failed: {}",
                path_ref.display(),
                e
            )
        });
    }
    crate::safe_delete::trash(path_ref)
}

/// `true` when `path` lives under `std::env::temp_dir()` (canonicalized
/// where possible). Conservative: if we can't resolve either side,
/// returns `false` and the caller routes through the production
/// trash path.
fn path_is_under_temp_dir(path: &Path) -> bool {
    let Ok(temp) = std::env::temp_dir().canonicalize() else {
        return false;
    };
    // canonicalize() requires the path to exist; we already checked
    // above. Fall back to as-is comparison if canonicalize fails for
    // reasons other than non-existence (e.g. permission).
    let resolved = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    resolved.starts_with(&temp)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static N: AtomicU64 = AtomicU64::new(0);

    fn scratch_dir() -> std::path::PathBuf {
        let n = N.fetch_add(1, Ordering::SeqCst);
        let pid = std::process::id();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "k2so-scratch-safe-trash-test-{pid}-{nanos}-{n}"
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn missing_path_is_ok() {
        let dir = scratch_dir();
        let missing = dir.join("nope");
        assert!(scratch_safe_trash(&missing).is_ok());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn scratch_dir_is_permanently_removed_no_trash() {
        // Path lives under temp_dir() → fs::remove_dir_all branch.
        // No AppleEvents call → no Touch ID prompt during cargo test.
        let dir = scratch_dir();
        let child = dir.join("payload");
        fs::create_dir_all(&child).unwrap();
        fs::write(child.join("file"), b"hi").unwrap();
        scratch_safe_trash(&child).unwrap();
        assert!(!child.exists(), "child should be permanently removed");
        // Parent still there; this helper only acts on the argument.
        assert!(dir.exists());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn scratch_file_is_permanently_removed() {
        let dir = scratch_dir();
        let f = dir.join("scratch.md");
        fs::write(&f, b"x").unwrap();
        scratch_safe_trash(&f).unwrap();
        assert!(!f.exists());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn path_is_under_temp_dir_recognizes_scratch() {
        let dir = scratch_dir();
        assert!(path_is_under_temp_dir(&dir));
        fs::remove_dir_all(&dir).ok();
    }
}
