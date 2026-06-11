//! Per-workspace dot-dir resolution (`<project>/.k2so` vs `<project>/.k2`).
//!
//! 0.40.0 rebrand, Q2 decision: the per-workspace directory stays
//! `.k2so/` BY DEFAULT through 0.40.x (it lives in USERS' repos — the
//! riskiest thing to rename), but `.k2/` is PREFERRED WHEN PRESENT so a
//! workspace can opt in by renaming the directory. The full default flip
//! + migration lands before 1.0.0.
//!
//! Every production path that anchors on the workspace dot-dir MUST go
//! through [`workspace_dot_dir`] — a literal `.join(".k2so")` is only
//! correct in (a) tests pinning the legacy-default branch, and (b) the
//! 0.37 unification migration, which operates on the historical layout
//! by definition.

use std::path::{Path, PathBuf};

/// Resolve the workspace dot-dir for a project root: `<root>/.k2` if it
/// exists as a real directory, else `<root>/.k2so` (the 0.x default —
/// also what gets created for new workspaces until the pre-1.0 flip).
pub fn workspace_dot_dir(project_root: impl AsRef<Path>) -> PathBuf {
    let root = project_root.as_ref();
    let k2 = root.join(".k2");
    if k2.is_dir() {
        k2
    } else {
        root.join(".k2so")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "k2-wsdir-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn defaults_to_k2so_when_neither_exists() {
        let root = tmp("fresh");
        assert_eq!(workspace_dot_dir(&root), root.join(".k2so"));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn defaults_to_k2so_when_only_k2so_exists() {
        let root = tmp("legacy");
        std::fs::create_dir_all(root.join(".k2so")).unwrap();
        assert_eq!(workspace_dot_dir(&root), root.join(".k2so"));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn prefers_k2_when_present() {
        let root = tmp("optin");
        std::fs::create_dir_all(root.join(".k2")).unwrap();
        assert_eq!(workspace_dot_dir(&root), root.join(".k2"));
        // Even if a stale .k2so also exists, .k2 wins.
        std::fs::create_dir_all(root.join(".k2so")).unwrap();
        assert_eq!(workspace_dot_dir(&root), root.join(".k2"));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_k2_file_is_not_a_dot_dir() {
        let root = tmp("notdir");
        std::fs::write(root.join(".k2"), b"not a dir").unwrap();
        assert_eq!(workspace_dot_dir(&root), root.join(".k2so"));
        std::fs::remove_dir_all(&root).unwrap();
    }
}
